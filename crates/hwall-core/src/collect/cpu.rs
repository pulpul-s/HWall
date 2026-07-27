use super::util::{add_string, list_dirs, read_trimmed, read_u64};
use crate::model::{
    Device, DeviceClass, Identification, PropertyValue, Sensor, SensorKind, SnapshotBuilder, Unit,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

pub(super) fn collect(builder: &mut SnapshotBuilder) {
    let content = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    if content.is_empty() && !Path::new("/sys/devices/system/cpu").exists() {
        return;
    }

    let records = parse_cpuinfo(&content);
    let first = records.first().cloned().unwrap_or_default();
    let model = first
        .get("model name")
        .or_else(|| first.get("hardware"))
        .or_else(|| first.get("processor"))
        .cloned()
        .unwrap_or_else(|| "CPU".to_owned());
    let vendor = first
        .get("vendor_id")
        .or_else(|| first.get("cpu implementer"))
        .cloned();

    let mut device = Device::new("cpu:0", DeviceClass::Cpu, model.clone());
    device.vendor = vendor;
    device.model = Some(model);
    device.driver = read_trimmed("/sys/devices/system/cpu/cpufreq/policy0/scaling_driver");

    if let Some(topology) = cpu_topology_from_sysfs(Path::new("/sys/devices/system/cpu"))
        .or_else(|| cpu_topology_from_cpuinfo(&records))
    {
        device
            .properties
            .insert("threads".to_owned(), topology.threads.into());
        device
            .properties
            .insert("cores".to_owned(), topology.cores.into());
        device
            .properties
            .insert("sockets".to_owned(), topology.sockets.into());
    }
    add_string(
        &mut device.properties,
        "microcode",
        first.get("microcode").cloned(),
    );
    add_string(
        &mut device.properties,
        "cpu_family",
        first.get("cpu family").cloned(),
    );
    add_string(
        &mut device.properties,
        "model_number",
        first.get("model").cloned(),
    );
    add_string(
        &mut device.properties,
        "stepping",
        first.get("stepping").cloned(),
    );
    add_string(
        &mut device.properties,
        "flags",
        first
            .get("flags")
            .or_else(|| first.get("features"))
            .cloned(),
    );

    collect_cache_properties(&mut device);
    collect_vulnerabilities(&mut device);
    add_dynamic(&mut device);
    builder.add_device(device);
}

pub(super) fn collect_dynamic(builder: &mut SnapshotBuilder) {
    let mut device = Device::new("cpu:0", DeviceClass::Cpu, "CPU");
    add_dynamic(&mut device);
    builder.add_device(device);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CpuTopology {
    threads: u64,
    cores: u64,
    sockets: u64,
}

fn cpu_topology_from_sysfs(root: &Path) -> Option<CpuTopology> {
    let logical_cpus = read_trimmed(root.join("online"))
        .map(|value| parse_cpu_list(&value))
        .filter(|cpus| !cpus.is_empty())
        .unwrap_or_else(|| {
            list_dirs(root)
                .into_iter()
                .filter_map(|path| {
                    let name = path.file_name()?.to_str()?;
                    let cpu = name.strip_prefix("cpu")?.parse::<u32>().ok()?;
                    let online = read_trimmed(path.join("online"));
                    (online.as_deref() != Some("0")).then_some(cpu)
                })
                .collect()
        });
    if logical_cpus.is_empty() {
        return None;
    }

    let mut packages = BTreeSet::new();
    let mut cores = BTreeSet::new();
    for cpu in &logical_cpus {
        let topology = root.join(format!("cpu{cpu}/topology"));
        let package =
            read_trimmed(topology.join("physical_package_id")).unwrap_or_else(|| "0".to_owned());
        let core = read_trimmed(topology.join("core_id")).unwrap_or_else(|| cpu.to_string());
        packages.insert(package.clone());
        cores.insert((package, core));
    }

    Some(CpuTopology {
        threads: logical_cpus.len() as u64,
        cores: cores.len() as u64,
        sockets: packages.len().max(1) as u64,
    })
}

fn cpu_topology_from_cpuinfo(records: &[BTreeMap<String, String>]) -> Option<CpuTopology> {
    if records.is_empty() {
        return None;
    }
    let mut packages = BTreeSet::new();
    let mut cores = BTreeSet::new();
    for record in records {
        let package = record
            .get("physical id")
            .cloned()
            .unwrap_or_else(|| "0".to_owned());
        let core = record.get("core id").cloned().unwrap_or_else(|| {
            record
                .get("processor")
                .cloned()
                .unwrap_or_else(|| "0".to_owned())
        });
        packages.insert(package.clone());
        cores.insert((package, core));
    }
    Some(CpuTopology {
        threads: records.len() as u64,
        cores: cores.len() as u64,
        sockets: packages.len().max(1) as u64,
    })
}

fn add_dynamic(device: &mut Device) {
    add_cpu_time_counters(device);
    let root = Path::new("/sys/devices/system/cpu/cpufreq");
    let mut all_policy_frequencies = Vec::new();
    let mut core_frequencies: BTreeMap<String, FrequencyAggregate> = BTreeMap::new();

    for policy in list_dirs(root) {
        let Some(policy_name) = policy.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !policy_name.starts_with("policy") {
            continue;
        }
        let Some(khz) = read_u64(policy.join("scaling_cur_freq"))
            .or_else(|| read_u64(policy.join("cpuinfo_cur_freq")))
        else {
            continue;
        };

        let frequency_hz = khz as f64 * 1_000.0;
        all_policy_frequencies.push(frequency_hz);
        let logical_cpus = read_trimmed(policy.join("affected_cpus"))
            .or_else(|| read_trimmed(policy.join("related_cpus")))
            .map(|value| parse_cpu_list(&value))
            .unwrap_or_default();
        let (key, label) = frequency_owner(policy_name, &logical_cpus);
        let aggregate = core_frequencies
            .entry(key)
            .or_insert_with(|| FrequencyAggregate::new(label));
        aggregate.values.push(frequency_hz);
        aggregate.policies.push(policy_name.to_owned());
        aggregate.logical_cpus.extend(logical_cpus);
    }

    if !all_policy_frequencies.is_empty() {
        let average =
            all_policy_frequencies.iter().sum::<f64>() / all_policy_frequencies.len() as f64;
        let mut sensor = Sensor::new(
            "cpu:0:frequency:average",
            "Average CPU frequency",
            SensorKind::Frequency,
            Unit::Hertz,
            Some(average),
            "/sys/devices/system/cpu/cpufreq/policy*/scaling_cur_freq",
            Identification::KernelLabel,
        );
        sensor.metadata.insert(
            "policies_sampled".to_owned(),
            (all_policy_frequencies.len() as u64).into(),
        );
        device.sensors.push(sensor);
    }

    for (key, mut aggregate) in core_frequencies {
        aggregate.logical_cpus.sort_unstable();
        aggregate.logical_cpus.dedup();
        aggregate.policies.sort();
        aggregate.policies.dedup();
        let value = aggregate.values.iter().sum::<f64>() / aggregate.values.len() as f64;
        let source = aggregate
            .policies
            .iter()
            .map(|policy| format!("/sys/devices/system/cpu/cpufreq/{policy}/scaling_cur_freq"))
            .collect::<Vec<_>>()
            .join(", ");
        let mut sensor = Sensor::new(
            format!("cpu:0:frequency:{key}"),
            aggregate.label,
            SensorKind::Frequency,
            Unit::Hertz,
            Some(value),
            source,
            Identification::KernelLabel,
        );
        sensor.metadata.insert(
            "policies".to_owned(),
            PropertyValue::Strings(aggregate.policies),
        );
        sensor.metadata.insert(
            "logical_cpus".to_owned(),
            PropertyValue::Strings(aggregate.logical_cpus.iter().map(u32::to_string).collect()),
        );
        if aggregate.values.len() > 1 {
            sensor
                .metadata
                .insert("aggregation".to_owned(), "average".into());
        }
        device.sensors.push(sensor);
    }

    add_string(
        &mut device.properties,
        "scaling_governor",
        read_trimmed(root.join("policy0/scaling_governor")),
    );
    add_string(
        &mut device.properties,
        "energy_performance_preference",
        read_trimmed(root.join("policy0/energy_performance_preference")),
    );
    add_string(
        &mut device.properties,
        "scaling_available_governors",
        read_trimmed(root.join("policy0/scaling_available_governors")),
    );
    add_frequency_property(
        device,
        "minimum_frequency_hz",
        read_u64(root.join("policy0/cpuinfo_min_freq"))
            .or_else(|| read_u64(root.join("policy0/scaling_min_freq"))),
    );
    add_frequency_property(
        device,
        "maximum_frequency_hz",
        read_u64(root.join("policy0/cpuinfo_max_freq"))
            .or_else(|| read_u64(root.join("policy0/scaling_max_freq"))),
    );
    add_frequency_property(
        device,
        "base_frequency_hz",
        read_u64(root.join("policy0/base_frequency")),
    );
    if let Some(boost) = read_u64(root.join("boost"))
        .or_else(|| read_u64("/sys/devices/system/cpu/cpu0/cpufreq/boost"))
    {
        device
            .properties
            .insert("boost_enabled".to_owned(), (boost != 0).into());
    }
}

fn add_cpu_time_counters(device: &mut Device) {
    let Ok(content) = fs::read_to_string("/proc/stat") else {
        return;
    };
    let Some(line) = content.lines().find(|line| line.starts_with("cpu ")) else {
        return;
    };
    let Ok(values) = line
        .split_whitespace()
        .skip(1)
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
    else {
        return;
    };
    if values.len() < 4 {
        return;
    }
    let user = values.first().copied().unwrap_or(0);
    let nice = values.get(1).copied().unwrap_or(0);
    let system = values.get(2).copied().unwrap_or(0);
    let idle = values.get(3).copied().unwrap_or(0);
    let iowait = values.get(4).copied().unwrap_or(0);
    let irq = values.get(5).copied().unwrap_or(0);
    let softirq = values.get(6).copied().unwrap_or(0);
    let steal = values.get(7).copied().unwrap_or(0);
    let total = user
        .saturating_add(nice)
        .saturating_add(system)
        .saturating_add(idle)
        .saturating_add(iowait)
        .saturating_add(irq)
        .saturating_add(softirq)
        .saturating_add(steal);
    device.counters.insert("cpu_total_ticks".to_owned(), total);
    device
        .counters
        .insert("cpu_idle_ticks".to_owned(), idle.saturating_add(iowait));
}

#[derive(Debug)]
struct FrequencyAggregate {
    label: String,
    values: Vec<f64>,
    policies: Vec<String>,
    logical_cpus: Vec<u32>,
}

impl FrequencyAggregate {
    fn new(label: String) -> Self {
        Self {
            label,
            values: Vec::new(),
            policies: Vec::new(),
            logical_cpus: Vec::new(),
        }
    }
}

fn frequency_owner(policy_name: &str, logical_cpus: &[u32]) -> (String, String) {
    let mut physical_cores = BTreeSet::new();
    for cpu in logical_cpus {
        let topology = Path::new("/sys/devices/system/cpu").join(format!("cpu{cpu}/topology"));
        let package = read_u64(topology.join("physical_package_id")).unwrap_or(0);
        let core = read_u64(topology.join("core_id")).unwrap_or(*cpu as u64);
        physical_cores.insert((package, core));
    }

    let mut core_iter = physical_cores.into_iter();
    if let (Some((package, core)), None) = (core_iter.next(), core_iter.next()) {
        let key = format!("socket{package}:core{core}");
        let label = if package == 0 {
            format!("Core {core} frequency")
        } else {
            format!("Socket {package} core {core} frequency")
        };
        return (key, label);
    }

    let suffix = policy_name.trim_start_matches("policy");
    (
        format!("policy{suffix}"),
        format!("CPU frequency policy {suffix}"),
    )
}

fn parse_cpu_list(value: &str) -> Vec<u32> {
    let mut cpus = Vec::new();
    for token in value.split(|character: char| character.is_ascii_whitespace() || character == ',')
    {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        if let Some((start, end)) = token.split_once('-') {
            let (Ok(start), Ok(end)) = (start.parse::<u32>(), end.parse::<u32>()) else {
                continue;
            };
            cpus.extend(start..=end);
        } else if let Ok(cpu) = token.parse::<u32>() {
            cpus.push(cpu);
        }
    }
    cpus.sort_unstable();
    cpus.dedup();
    cpus
}

fn add_frequency_property(device: &mut Device, key: &str, value_khz: Option<u64>) {
    if let Some(value_khz) = value_khz {
        device
            .properties
            .insert(key.to_owned(), (value_khz as f64 * 1_000.0).into());
    }
}

fn collect_cache_properties(device: &mut Device) {
    let root = Path::new("/sys/devices/system/cpu/cpu0/cache");
    let mut caches = Vec::new();
    for index in list_dirs(root) {
        let level = read_trimmed(index.join("level"));
        let kind = read_trimmed(index.join("type"));
        let size = read_trimmed(index.join("size"));
        if let (Some(level), Some(kind), Some(size)) = (level, kind, size) {
            caches.push(format!("L{level} {kind}: {size}"));
        }
    }
    if !caches.is_empty() {
        device
            .properties
            .insert("cache_hierarchy".to_owned(), PropertyValue::Strings(caches));
    }
}

fn collect_vulnerabilities(device: &mut Device) {
    let root = Path::new("/sys/devices/system/cpu/vulnerabilities");
    for path in super::util::list_entries(root) {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if let Some(value) = read_trimmed(&path) {
            device
                .properties
                .insert(format!("vulnerability_{name}"), value.into());
        }
    }
}

fn parse_cpuinfo(content: &str) -> Vec<BTreeMap<String, String>> {
    content
        .split("\n\n")
        .filter_map(|section| {
            let mut record = BTreeMap::new();
            for line in section.lines() {
                let Some((key, value)) = line.split_once(':') else {
                    continue;
                };
                record.insert(key.trim().to_ascii_lowercase(), value.trim().to_owned());
            }
            (!record.is_empty()).then_some(record)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cpu_lists_and_ranges() {
        assert_eq!(parse_cpu_list("0 1 8 9"), vec![0, 1, 8, 9]);
        assert_eq!(parse_cpu_list("0-3,8-9"), vec![0, 1, 2, 3, 8, 9]);
        assert_eq!(parse_cpu_list("2 2 3"), vec![2, 3]);
    }

    #[test]
    fn reads_online_topology_from_sysfs() {
        let root = std::env::temp_dir().join(format!("hwall-cpu-topology-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("online"), "0-3").unwrap();
        for (cpu, core) in [(0, 0), (1, 0), (2, 1), (3, 1)] {
            let topology = root.join(format!("cpu{cpu}/topology"));
            std::fs::create_dir_all(&topology).unwrap();
            std::fs::write(topology.join("physical_package_id"), "0").unwrap();
            std::fs::write(topology.join("core_id"), core.to_string()).unwrap();
        }

        assert_eq!(
            cpu_topology_from_sysfs(&root),
            Some(CpuTopology {
                threads: 4,
                cores: 2,
                sockets: 1,
            })
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn falls_back_to_cpuinfo_topology() {
        let records = parse_cpuinfo(
            "processor: 0\nphysical id: 0\ncore id: 0\n\n\
             processor: 1\nphysical id: 0\ncore id: 0\n\n\
             processor: 2\nphysical id: 1\ncore id: 0\n",
        );
        assert_eq!(
            cpu_topology_from_cpuinfo(&records),
            Some(CpuTopology {
                threads: 3,
                cores: 2,
                sockets: 2,
            })
        );
    }

    #[test]
    fn empty_cpu_list_uses_policy_identity() {
        assert_eq!(
            frequency_owner("policy7", &[]),
            ("policy7".to_owned(), "CPU frequency policy 7".to_owned())
        );
    }
}
