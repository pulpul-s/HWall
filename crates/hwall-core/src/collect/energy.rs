//! Optional power telemetry derived from Linux energy counters.
//!
//! Powercap is preferred when readable. Perf PMU events fill missing domains,
//! including AMD physical-core counters. Each source fails independently.

use super::perf_event::{PerfCounter, PerfEvent};
use super::util::{list_dirs, read_trimmed, read_u64};
use crate::model::{Device, DeviceClass, Identification, Sensor, SensorKind, Snapshot, Unit};
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const POWERCAP_ROOT: &str = "/sys/class/powercap";
const PMU_ROOT: &str = "/sys/bus/event_source/devices";
const CPU_ROOT: &str = "/sys/devices/system/cpu";
const MAX_INTERVAL: Duration = Duration::from_secs(300);
const DERIVED_BY: &str = "hwall-energy";

#[derive(Debug, Default)]
pub(crate) struct EnergyCollector {
    sources: BTreeMap<Domain, Source>,
    expected_cores: BTreeMap<u32, usize>,
}

impl EnergyCollector {
    pub(crate) fn new() -> Self {
        let mut collector = Self::default();
        collector.refresh_sources();
        collector
    }

    pub(crate) fn refresh_sources(&mut self) {
        let topology = cpu_topology(Path::new(CPU_ROOT));
        self.expected_cores.clear();
        for cpu in physical_cores(&topology) {
            *self.expected_cores.entry(cpu.package).or_default() += 1;
        }

        let mut specs = BTreeMap::new();
        discover_powercap(Path::new(POWERCAP_ROOT), &mut specs);
        discover_perf(Path::new(PMU_ROOT), &topology, &mut specs);
        for (domain, spec) in specs {
            let Entry::Vacant(entry) = self.sources.entry(domain) else {
                continue;
            };
            if let Ok(source) = Source::open(domain, spec) {
                entry.insert(source);
            }
        }
    }

    pub(crate) fn sample(&mut self, snapshot: &mut Snapshot) {
        let now = Instant::now();
        let mut readings = self
            .sources
            .values_mut()
            .filter_map(|source| source.sample(now))
            .collect::<Vec<_>>();
        add_core_totals(&mut readings, &self.expected_cores);
        for reading in readings {
            attach(snapshot, reading);
        }
    }

    pub(crate) fn clear_sensors(snapshot: &mut Snapshot) {
        for device in &mut snapshot.devices {
            device
                .sensors
                .retain(|sensor| sensor.metadata_str("derived_by") != Some(DERIVED_BY));
        }
    }
}

#[derive(Debug)]
struct Source {
    domain: Domain,
    backend: &'static str,
    source: String,
    reader: Reader,
    previous: Option<(u64, Instant)>,
}

impl Source {
    fn open(domain: Domain, spec: SourceSpec) -> io::Result<Self> {
        match spec {
            SourceSpec::Powercap { path, maximum } => {
                read_u64(&path).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "energy counter is unreadable",
                    )
                })?;
                Ok(Self {
                    domain,
                    backend: "powercap",
                    source: path.to_string_lossy().into_owned(),
                    reader: Reader::Powercap { path, maximum },
                    previous: None,
                })
            }
            SourceSpec::Perf { pmu, event, cpu } => Ok(Self {
                domain,
                backend: "perf_event",
                source: format!(
                    "/sys/bus/event_source/devices/{pmu}/events/{} (CPU {cpu})",
                    event.name
                ),
                reader: Reader::Perf(event.open(cpu)?),
                previous: None,
            }),
        }
    }

    fn sample(&mut self, now: Instant) -> Option<Reading> {
        let (current, scale, maximum) = match self.reader.read() {
            Ok(value) => value,
            Err(_) => {
                self.previous = None;
                return None;
            }
        };
        let (previous, previous_at) = self.previous.replace((current, now))?;
        let elapsed = now.saturating_duration_since(previous_at);
        let watts = average_power(previous, current, scale, maximum, elapsed)?;
        Some(Reading {
            domain: self.domain,
            watts,
            source: self.source.clone(),
            backend: self.backend,
            aggregation: "interval_average",
        })
    }
}

#[derive(Debug)]
enum Reader {
    Powercap { path: PathBuf, maximum: Option<u64> },
    Perf(PerfCounter),
}

impl Reader {
    fn read(&self) -> io::Result<(u64, f64, Option<u64>)> {
        match self {
            Self::Powercap { path, maximum } => read_u64(path)
                .map(|value| (value, 1.0 / 1_000_000.0, *maximum))
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::UnexpectedEof, "energy counter is unreadable")
                }),
            Self::Perf(counter) => counter.read().map(|(value, scale)| (value, scale, None)),
        }
    }
}

#[derive(Debug, Clone)]
enum SourceSpec {
    Powercap {
        path: PathBuf,
        maximum: Option<u64>,
    },
    Perf {
        pmu: String,
        event: PerfEvent,
        cpu: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Domain {
    Package(u32),
    Cores(u32),
    Core { package: u32, core: u32 },
    Uncore(u32),
    Dram(u32),
    Gpu(u32),
    Platform(u32),
}

impl Domain {
    fn package(self) -> u32 {
        match self {
            Self::Package(package)
            | Self::Cores(package)
            | Self::Core { package, .. }
            | Self::Uncore(package)
            | Self::Dram(package)
            | Self::Gpu(package)
            | Self::Platform(package) => package,
        }
    }

    fn key(self) -> String {
        match self {
            Self::Package(package) => format!("package:{package}"),
            Self::Cores(package) => format!("cores:{package}"),
            Self::Core { package, core } => format!("core:{package}:{core}"),
            Self::Uncore(package) => format!("uncore:{package}"),
            Self::Dram(package) => format!("dram:{package}"),
            Self::Gpu(package) => format!("gpu:{package}"),
            Self::Platform(package) => format!("platform:{package}"),
        }
    }

    fn metadata(self) -> &'static str {
        match self {
            Self::Package(_) => "package",
            Self::Cores(_) => "cores",
            Self::Core { .. } => "core",
            Self::Uncore(_) => "uncore",
            Self::Dram(_) => "dram",
            Self::Gpu(_) => "gpu",
            Self::Platform(_) => "platform",
        }
    }
}

#[derive(Debug, Clone)]
struct Reading {
    domain: Domain,
    watts: f64,
    source: String,
    backend: &'static str,
    aggregation: &'static str,
}

fn discover_powercap(root: &Path, specs: &mut BTreeMap<Domain, SourceSpec>) {
    for path in list_dirs(root) {
        discover_powercap_tree(&path, None, specs);
    }
}

fn discover_powercap_tree(
    path: &Path,
    parent_package: Option<u32>,
    specs: &mut BTreeMap<Domain, SourceSpec>,
) {
    let name = read_trimmed(path.join("name"));
    let package = name
        .as_deref()
        .and_then(package_number)
        .or(parent_package)
        .unwrap_or(0);
    if let Some(domain) = name.as_deref().and_then(|name| domain(name, package)) {
        let energy = path.join("energy_uj");
        if read_u64(&energy).is_some() {
            specs.entry(domain).or_insert(SourceSpec::Powercap {
                path: energy,
                maximum: read_u64(path.join("max_energy_range_uj")),
            });
        }
    }
    for child in list_dirs(path) {
        if child.join("name").is_file() {
            discover_powercap_tree(&child, Some(package), specs);
        }
    }
}

fn discover_perf(root: &Path, topology: &[CpuLocation], specs: &mut BTreeMap<Domain, SourceSpec>) {
    for pmu_path in list_dirs(root) {
        let Some(pmu) = pmu_path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
        else {
            continue;
        };
        for event in PerfEvent::discover(&pmu_path) {
            let token = event.name.trim_start_matches("energy-");
            if pmu == "power_core" && token == "core" {
                for cpu in physical_cores(topology) {
                    let domain = Domain::Core {
                        package: cpu.package,
                        core: cpu.core,
                    };
                    specs.entry(domain).or_insert(SourceSpec::Perf {
                        pmu: pmu.clone(),
                        event: event.clone(),
                        cpu: cpu.cpu,
                    });
                }
                continue;
            }
            let Some(kind) = domain_kind(token) else {
                continue;
            };
            for cpu in representative_cpus(&pmu_path, topology) {
                let domain = kind.with_package(cpu.package);
                specs.entry(domain).or_insert(SourceSpec::Perf {
                    pmu: pmu.clone(),
                    event: event.clone(),
                    cpu: cpu.cpu,
                });
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum DomainKind {
    Package,
    Cores,
    Uncore,
    Dram,
    Gpu,
    Platform,
}

impl DomainKind {
    fn with_package(self, package: u32) -> Domain {
        match self {
            Self::Package => Domain::Package(package),
            Self::Cores => Domain::Cores(package),
            Self::Uncore => Domain::Uncore(package),
            Self::Dram => Domain::Dram(package),
            Self::Gpu => Domain::Gpu(package),
            Self::Platform => Domain::Platform(package),
        }
    }
}

fn domain(name: &str, package: u32) -> Option<Domain> {
    let name = name.trim().to_ascii_lowercase();
    if name.starts_with("package-") || name.starts_with("package_") {
        Some(Domain::Package(package))
    } else {
        Some(domain_kind(&name)?.with_package(package))
    }
}

fn domain_kind(name: &str) -> Option<DomainKind> {
    match name {
        "pkg" | "package" => Some(DomainKind::Package),
        "core" | "cores" => Some(DomainKind::Cores),
        "uncore" => Some(DomainKind::Uncore),
        "ram" | "dram" => Some(DomainKind::Dram),
        "gpu" => Some(DomainKind::Gpu),
        "psys" | "platform" => Some(DomainKind::Platform),
        _ => None,
    }
}

fn package_number(name: &str) -> Option<u32> {
    for marker in ["package-", "package_"] {
        let Some(value) = name.split_once(marker).map(|(_, value)| value) else {
            continue;
        };
        let digits = value
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>();
        if !digits.is_empty() {
            return digits.parse().ok();
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CpuLocation {
    cpu: u32,
    package: u32,
    core: u32,
}

fn cpu_topology(root: &Path) -> Vec<CpuLocation> {
    let mut cpus = read_trimmed(root.join("online"))
        .map(|value| parse_cpu_list(&value))
        .unwrap_or_default();
    if cpus.is_empty() {
        cpus = list_dirs(root)
            .into_iter()
            .filter_map(|path| {
                let cpu = path
                    .file_name()?
                    .to_str()?
                    .strip_prefix("cpu")?
                    .parse::<u32>()
                    .ok()?;
                (read_trimmed(path.join("online")).as_deref() != Some("0")).then_some(cpu)
            })
            .collect();
    }

    cpus.into_iter()
        .map(|cpu| {
            let topology = root.join(format!("cpu{cpu}/topology"));
            let package = read_u64(topology.join("physical_package_id"))
                .and_then(|value| u32::try_from(value).ok())
                .unwrap_or(0);
            let core = read_u64(topology.join("core_id"))
                .and_then(|value| u32::try_from(value).ok())
                .unwrap_or(cpu);
            CpuLocation { cpu, package, core }
        })
        .collect()
}

fn physical_cores(topology: &[CpuLocation]) -> Vec<CpuLocation> {
    let mut cores = BTreeMap::new();
    for cpu in topology {
        cores.entry((cpu.package, cpu.core)).or_insert(*cpu);
    }
    cores.into_values().collect()
}

fn representative_cpus(pmu: &Path, topology: &[CpuLocation]) -> Vec<CpuLocation> {
    let mask = read_trimmed(pmu.join("cpumask"))
        .or_else(|| read_trimmed(pmu.join("cpus")))
        .map(|value| parse_cpu_list(&value))
        .unwrap_or_default()
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut packages = BTreeMap::new();
    for cpu in topology {
        if mask.is_empty() || mask.contains(&cpu.cpu) {
            packages.entry(cpu.package).or_insert(*cpu);
        }
    }
    if packages.is_empty() {
        let fallback = topology.first().copied().unwrap_or_else(|| {
            let cpu = mask.iter().next().copied().unwrap_or(0);
            CpuLocation {
                cpu,
                package: 0,
                core: cpu,
            }
        });
        packages.insert(fallback.package, fallback);
    }
    packages.into_values().collect()
}

fn parse_cpu_list(value: &str) -> Vec<u32> {
    let mut cpus = BTreeSet::new();
    for token in value
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        if let Some((start, end)) = token.split_once('-') {
            let (Ok(start), Ok(end)) = (start.parse::<u32>(), end.parse::<u32>()) else {
                continue;
            };
            cpus.extend(start..=end);
        } else if let Ok(cpu) = token.parse::<u32>() {
            cpus.insert(cpu);
        }
    }
    cpus.into_iter().collect()
}

fn average_power(
    previous: u64,
    current: u64,
    scale: f64,
    maximum: Option<u64>,
    elapsed: Duration,
) -> Option<f64> {
    if elapsed.is_zero() || elapsed > MAX_INTERVAL || !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let delta = counter_delta(previous, current, maximum)?;
    let watts = delta as f64 * scale / elapsed.as_secs_f64();
    watts.is_finite().then_some(watts)
}

fn counter_delta(previous: u64, current: u64, maximum: Option<u64>) -> Option<u64> {
    if current >= previous {
        Some(current - previous)
    } else {
        let maximum = maximum?;
        (previous <= maximum && current <= maximum).then_some(maximum - previous + current)
    }
}

fn add_core_totals(readings: &mut Vec<Reading>, expected: &BTreeMap<u32, usize>) {
    let measured = readings
        .iter()
        .filter_map(|reading| match reading.domain {
            Domain::Cores(package) => Some(package),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let mut cores: BTreeMap<u32, Vec<&Reading>> = BTreeMap::new();
    for reading in &*readings {
        if let Domain::Core { package, .. } = reading.domain {
            cores.entry(package).or_default().push(reading);
        }
    }

    let mut totals = Vec::new();
    for (package, values) in cores {
        if measured.contains(&package) || expected.get(&package).copied() != Some(values.len()) {
            continue;
        }
        totals.push(Reading {
            domain: Domain::Cores(package),
            watts: values.iter().map(|reading| reading.watts).sum(),
            source: values
                .iter()
                .map(|reading| reading.source.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            backend: "perf_event",
            aggregation: "sum_of_physical_cores",
        });
    }
    readings.extend(totals);
}

fn attach(snapshot: &mut Snapshot, reading: Reading) {
    let target = target_device(snapshot, reading.domain);
    let (class, name) = target_identity(reading.domain);
    let device = ensure_device(snapshot, &target, class, name);
    let mut sensor = Sensor::new(
        format!("{target}:power:{}", reading.domain.key()),
        label(reading.domain),
        SensorKind::Power,
        Unit::Watt,
        Some(reading.watts),
        reading.source,
        Identification::KnownDriverMapping,
    );
    sensor
        .metadata
        .insert("derived_by".to_owned(), DERIVED_BY.into());
    sensor
        .metadata
        .insert("energy_backend".to_owned(), reading.backend.into());
    sensor
        .metadata
        .insert("energy_domain".to_owned(), reading.domain.metadata().into());
    sensor
        .metadata
        .insert("aggregation".to_owned(), reading.aggregation.into());
    sensor.metadata.insert(
        "package_id".to_owned(),
        u64::from(reading.domain.package()).into(),
    );
    if let Domain::Core { core, .. } = reading.domain {
        sensor
            .metadata
            .insert("core_id".to_owned(), u64::from(core).into());
    }
    device.sensors.push(sensor);
}

fn target_device(snapshot: &Snapshot, domain: Domain) -> String {
    match domain {
        Domain::Package(_) | Domain::Cores(_) | Domain::Core { .. } | Domain::Uncore(_) => {
            "cpu:0".to_owned()
        }
        Domain::Dram(_) => "memory:system".to_owned(),
        Domain::Platform(_) => "system:0".to_owned(),
        Domain::Gpu(_) => integrated_gpu(snapshot).unwrap_or_else(|| "cpu:0".to_owned()),
    }
}

fn integrated_gpu(snapshot: &Snapshot) -> Option<String> {
    let mut amdgpus = Vec::new();
    for device in &snapshot.devices {
        if device.class != DeviceClass::Gpu {
            continue;
        }
        match device.driver.as_deref() {
            Some("i915" | "xe") => return Some(device.id.clone()),
            Some("amdgpu") => amdgpus.push(device.id.clone()),
            _ => {}
        }
    }
    if amdgpus.len() == 1 {
        amdgpus.pop()
    } else {
        None
    }
}

fn target_identity(domain: Domain) -> (DeviceClass, &'static str) {
    match domain {
        Domain::Dram(_) => (DeviceClass::Memory, "System memory"),
        Domain::Platform(_) => (DeviceClass::System, "Linux system"),
        _ => (DeviceClass::Cpu, "CPU"),
    }
}

fn ensure_device<'a>(
    snapshot: &'a mut Snapshot,
    id: &str,
    class: DeviceClass,
    name: &str,
) -> &'a mut Device {
    if let Some(index) = snapshot.devices.iter().position(|device| device.id == id) {
        return &mut snapshot.devices[index];
    }
    let index = snapshot.devices.len();
    snapshot.devices.push(Device::new(id, class, name));
    &mut snapshot.devices[index]
}

fn label(domain: Domain) -> String {
    match domain {
        Domain::Package(0) => "CPU package power".to_owned(),
        Domain::Package(package) => format!("CPU package {package} power"),
        Domain::Cores(0) => "CPU cores power".to_owned(),
        Domain::Cores(package) => format!("CPU package {package} cores power"),
        Domain::Core { package: 0, core } => format!("Core {core} power"),
        Domain::Core { package, core } => format!("Package {package} core {core} power"),
        Domain::Uncore(0) => "CPU uncore power".to_owned(),
        Domain::Uncore(package) => format!("CPU package {package} uncore power"),
        Domain::Dram(0) => "DRAM power".to_owned(),
        Domain::Dram(package) => format!("DRAM package {package} power"),
        Domain::Gpu(0) => "Integrated GPU power".to_owned(),
        Domain::Gpu(package) => format!("Integrated GPU package {package} power"),
        Domain::Platform(0) => "Platform power".to_owned(),
        Domain::Platform(package) => format!("Platform package {package} power"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::PropertyValue;

    #[test]
    fn parses_cpu_lists() {
        assert_eq!(parse_cpu_list("0-3,8,10-11"), vec![0, 1, 2, 3, 8, 10, 11]);
    }

    #[test]
    fn maps_common_energy_domains() {
        assert!(matches!(domain_kind("pkg"), Some(DomainKind::Package)));
        assert!(matches!(domain_kind("cores"), Some(DomainKind::Cores)));
        assert!(matches!(domain_kind("ram"), Some(DomainKind::Dram)));
        assert_eq!(package_number("package-12"), Some(12));
    }

    #[test]
    fn selects_one_logical_cpu_per_physical_core() {
        let topology = vec![
            CpuLocation {
                cpu: 0,
                package: 0,
                core: 0,
            },
            CpuLocation {
                cpu: 8,
                package: 0,
                core: 0,
            },
            CpuLocation {
                cpu: 1,
                package: 0,
                core: 1,
            },
        ];
        assert_eq!(physical_cores(&topology).len(), 2);
        assert_eq!(physical_cores(&topology)[0].cpu, 0);
    }

    #[test]
    fn derives_interval_average_power_and_handles_wrap() {
        assert_eq!(
            average_power(100, 175, 1.0, None, Duration::from_secs(3)),
            Some(25.0)
        );
        assert_eq!(
            average_power(950, 25, 1.0, Some(1_000), Duration::from_secs(3)),
            Some(25.0)
        );
        assert_eq!(
            average_power(950, 25, 1.0, None, Duration::from_secs(3)),
            None
        );
        assert_eq!(average_power(100, 175, 1.0, None, Duration::ZERO), None);
        assert_eq!(
            average_power(100, 175, 1.0, None, MAX_INTERVAL + Duration::from_secs(1)),
            None
        );
    }

    #[test]
    fn sums_only_complete_physical_core_sets() {
        let readings = || {
            vec![
                Reading {
                    domain: Domain::Core {
                        package: 0,
                        core: 0,
                    },
                    watts: 1.5,
                    source: "core0".to_owned(),
                    backend: "perf_event",
                    aggregation: "interval_average",
                },
                Reading {
                    domain: Domain::Core {
                        package: 0,
                        core: 1,
                    },
                    watts: 2.5,
                    source: "core1".to_owned(),
                    backend: "perf_event",
                    aggregation: "interval_average",
                },
            ]
        };
        let mut complete = readings();
        add_core_totals(&mut complete, &BTreeMap::from([(0, 2)]));
        assert_eq!(complete.len(), 3);
        assert_eq!(complete[2].watts, 4.0);

        let mut incomplete = readings();
        add_core_totals(&mut incomplete, &BTreeMap::from([(0, 3)]));
        assert_eq!(incomplete.len(), 2);
    }

    #[test]
    fn attaches_package_power_to_the_cpu() {
        let mut snapshot = Snapshot::new();
        snapshot
            .devices
            .push(Device::new("cpu:0", DeviceClass::Cpu, "Processor"));
        attach(
            &mut snapshot,
            Reading {
                domain: Domain::Package(0),
                watts: 42.0,
                source: "test".to_owned(),
                backend: "perf_event",
                aggregation: "interval_average",
            },
        );
        let sensor = &snapshot.devices[0].sensors[0];
        assert_eq!(sensor.label, "CPU package power");
        assert_eq!(sensor.value, Some(42.0));
        assert_eq!(sensor.unit, Unit::Watt);
        assert_eq!(
            sensor.metadata.get("energy_backend"),
            Some(&PropertyValue::String("perf_event".to_owned()))
        );
    }
}
