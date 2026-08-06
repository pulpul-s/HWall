//! Optional APERF/MPERF-derived effective CPU clocks.
//!
//! The x86 implementation reads a fixed set of sysfs files and keeps one
//! grouped perf event open per online logical CPU. Other architectures use a
//! no-op implementation.

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
mod platform {
    use super::super::perf_event::{PerfCounterGroup, PerfGroupReading, RawPerfEvent};
    use crate::model::{
        CollectorId, Device, DeviceClass, Identification, PropertyValue, ReadingFreshness, Sensor,
        SensorKind, Snapshot, Unit,
    };
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs::{self, File};
    use std::io::{self, Read};
    use std::path::Path;
    use std::time::{Duration, Instant};

    const MSR_PMU: &str = "/sys/bus/event_source/devices/msr";
    const CPU_PRESENT: &str = "/sys/devices/system/cpu/present";
    const CPU_ONLINE: &str = "/sys/devices/system/cpu/online";
    const PROC_SELF_FD: &str = "/proc/self/fd";
    const MAX_SYSFS_VALUE_BYTES: u64 = 4 * 1024;
    const MAX_CPU_COUNT: usize = 8_192;
    const MAX_CPU_ID: u32 = 65_535;
    const PERF_FDS_PER_CPU: u128 = 2;
    const FD_HEADROOM: u128 = 128;
    const MAX_INTERVAL: Duration = Duration::from_secs(300);
    const MAX_EFFECTIVE_HZ: f64 = 100_000_000_000.0;
    const SOURCE_PREFIX: &str = "/sys/bus/event_source/devices/msr/events";

    #[derive(Debug)]
    pub(crate) struct EffectiveClockCollector {
        events: Option<EventPair>,
        cpus: BTreeMap<u32, CpuState>,
        activated: bool,
        blocked_until_full_discovery: bool,
        last_average: Option<f64>,
        last_average_updated_unix_ms: Option<u128>,
    }

    impl EffectiveClockCollector {
        pub(crate) fn new() -> Self {
            Self {
                events: discover_events(),
                cpus: BTreeMap::new(),
                activated: false,
                blocked_until_full_discovery: false,
                last_average: None,
                last_average_updated_unix_ms: None,
            }
        }

        pub(crate) fn sample(&mut self, snapshot: &mut Snapshot, full_discovery: bool) {
            if full_discovery {
                self.refresh_capability();
                self.allow_full_discovery_retries();
            }
            let Some(events) = self.events else {
                if self.activated {
                    snapshot.mark_collector_succeeded(CollectorId::EffectiveClock);
                }
                return;
            };

            let Ok(topology) = read_topology() else {
                for state in self.cpus.values_mut() {
                    state.previous = None;
                }
                if self.activated {
                    snapshot.mark_collector_failed(CollectorId::EffectiveClock);
                }
                return;
            };

            self.cpus.retain(|cpu, _| topology.present.contains(cpu));
            for (cpu, state) in &mut self.cpus {
                if !topology.online.contains(cpu) {
                    state.group = None;
                    state.previous = None;
                    state.retry = RetryPolicy::NextSample;
                }
            }

            if self.blocked_until_full_discovery && !full_discovery {
                self.attach_blocked(snapshot, &topology);
                return;
            }

            let will_open_counters = topology.online.iter().any(|cpu| {
                self.cpus.get(cpu).is_none_or(|state| {
                    state.group.is_none() && state.retry.waiting_freshness().is_none()
                })
            });
            if will_open_counters {
                let open_groups = self
                    .cpus
                    .values()
                    .filter(|state| state.group.is_some())
                    .count();
                if !descriptor_budget_allows(topology.online.len(), open_groups) {
                    self.block_until_full_discovery();
                    self.attach_blocked(snapshot, &topology);
                    return;
                }
            }

            let captured_at = snapshot.captured_at_unix_ms;
            let mut readings = Vec::with_capacity(topology.present.len());
            for cpu in &topology.present {
                let sampled = {
                    let state = self.cpus.entry(*cpu).or_default();
                    sample_cpu(
                        state,
                        *cpu,
                        topology.online.contains(cpu),
                        events,
                        captured_at,
                    )
                    .map(|freshness| cpu_reading(*cpu, state, freshness))
                };
                match sampled {
                    Ok(reading) => readings.push(reading),
                    Err(GlobalCounterFailure) => {
                        self.block_until_full_discovery();
                        self.attach_blocked(snapshot, &topology);
                        return;
                    }
                }
            }

            self.activated |= self.cpus.values().any(|state| state.group.is_some());
            if !self.activated {
                return;
            }

            snapshot.mark_collector_succeeded(CollectorId::EffectiveClock);
            attach_readings(snapshot, &readings);
            self.attach_average(snapshot, &readings);
        }

        fn refresh_capability(&mut self) {
            let discovered = discover_events();
            if discovered == self.events {
                return;
            }
            self.events = discovered;
            self.blocked_until_full_discovery = false;
            for state in self.cpus.values_mut() {
                state.group = None;
                state.previous = None;
                state.retry = RetryPolicy::NextSample;
            }
        }

        fn allow_full_discovery_retries(&mut self) {
            self.blocked_until_full_discovery = false;
            for state in self.cpus.values_mut() {
                if state.group.is_none() {
                    state.retry = state.retry.after_full_discovery();
                }
            }
        }

        fn block_until_full_discovery(&mut self) {
            self.blocked_until_full_discovery = true;
            for state in self.cpus.values_mut() {
                state.group = None;
                state.previous = None;
                state.retry = RetryPolicy::UnavailableUntilFullDiscovery;
            }
        }

        fn attach_blocked(&mut self, snapshot: &mut Snapshot, topology: &CpuTopology) {
            if !self.activated {
                return;
            }
            let mut readings = Vec::with_capacity(topology.present.len());
            for cpu in &topology.present {
                let state = self.cpus.entry(*cpu).or_default();
                let freshness = if topology.online.contains(cpu) {
                    ReadingFreshness::Unavailable
                } else {
                    ReadingFreshness::Offline
                };
                readings.push(cpu_reading(*cpu, state, freshness));
            }
            snapshot.mark_collector_succeeded(CollectorId::EffectiveClock);
            attach_readings(snapshot, &readings);
            self.attach_average(snapshot, &readings);
        }

        fn attach_average(&mut self, snapshot: &mut Snapshot, readings: &[CpuReading]) {
            let aggregate = aggregate_readings(readings);
            if let Some(average) = aggregate.value {
                self.last_average = Some(average);
                self.last_average_updated_unix_ms = Some(snapshot.captured_at_unix_ms);
            }

            let mut sensor = effective_sensor(
                "cpu:0:effective_clock:average".to_owned(),
                "Average effective clock".to_owned(),
                aggregate.value.or(self.last_average),
                format!("{SOURCE_PREFIX}/aperf, {SOURCE_PREFIX}/mperf"),
                aggregate.freshness,
                self.last_average_updated_unix_ms,
            );
            sensor
                .metadata
                .insert("aggregation".to_owned(), "arithmetic_mean".into());
            sensor.metadata.insert(
                "logical_cpus_sampled".to_owned(),
                PropertyValue::Unsigned(aggregate.measured as u64),
            );
            sensor.metadata.insert(
                "logical_cpus_included".to_owned(),
                PropertyValue::Unsigned(aggregate.included as u64),
            );
            sensor.metadata.insert(
                "logical_cpus_present".to_owned(),
                PropertyValue::Unsigned(readings.len() as u64),
            );
            cpu_device(snapshot).sensors.push(sensor);
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct EventPair {
        aperf: RawPerfEvent,
        mperf: RawPerfEvent,
    }

    #[derive(Debug, Default)]
    struct CpuState {
        group: Option<PerfCounterGroup>,
        previous: Option<CounterBaseline>,
        last_value: Option<f64>,
        last_updated_unix_ms: Option<u128>,
        retry: RetryPolicy,
    }

    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    enum RetryPolicy {
        #[default]
        NextSample,
        RecoveryNextSample,
        StaleUntilFullDiscovery,
        UnavailableUntilFullDiscovery,
    }

    impl RetryPolicy {
        fn waiting_freshness(self) -> Option<ReadingFreshness> {
            match self {
                Self::StaleUntilFullDiscovery => Some(ReadingFreshness::Stale),
                Self::UnavailableUntilFullDiscovery => Some(ReadingFreshness::Unavailable),
                Self::NextSample | Self::RecoveryNextSample => None,
            }
        }

        fn after_full_discovery(self) -> Self {
            match self {
                Self::StaleUntilFullDiscovery => Self::RecoveryNextSample,
                Self::UnavailableUntilFullDiscovery => Self::NextSample,
                retry => retry,
            }
        }
    }

    #[derive(Debug, Clone, Copy)]
    struct CounterBaseline {
        reading: PerfGroupReading,
        sampled_at: Instant,
    }

    #[derive(Debug, Clone, Copy)]
    struct CpuReading {
        cpu: u32,
        value: Option<f64>,
        last_updated_unix_ms: Option<u128>,
        freshness: ReadingFreshness,
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct AggregateReading {
        value: Option<f64>,
        freshness: ReadingFreshness,
        measured: usize,
        included: usize,
    }

    #[derive(Debug)]
    struct CpuTopology {
        present: BTreeSet<u32>,
        online: BTreeSet<u32>,
    }

    #[derive(Debug, Clone, Copy)]
    struct GlobalCounterFailure;

    fn sample_cpu(
        state: &mut CpuState,
        cpu: u32,
        online: bool,
        events: EventPair,
        captured_at: u128,
    ) -> Result<ReadingFreshness, GlobalCounterFailure> {
        if !online {
            state.group = None;
            state.previous = None;
            state.retry = RetryPolicy::NextSample;
            return Ok(ReadingFreshness::Offline);
        }

        if state.group.is_none() {
            if let Some(freshness) = state.retry.waiting_freshness() {
                return Ok(freshness);
            }
            let recovering = state.retry == RetryPolicy::RecoveryNextSample;
            match PerfCounterGroup::open(cpu, events.aperf, events.mperf) {
                Ok(group) => {
                    state.group = Some(group);
                    state.previous = None;
                }
                Err(error) if is_global_open_failure(&error) => {
                    return Err(GlobalCounterFailure);
                }
                Err(_) => {
                    state.retry = if recovering && state.last_value.is_some() {
                        RetryPolicy::StaleUntilFullDiscovery
                    } else {
                        RetryPolicy::UnavailableUntilFullDiscovery
                    };
                    let freshness = state
                        .retry
                        .waiting_freshness()
                        .unwrap_or(ReadingFreshness::Unavailable);
                    return Ok(freshness);
                }
            }
        }

        let recovering = state.retry == RetryPolicy::RecoveryNextSample;
        let read_result = {
            let Some(group) = state.group.as_ref() else {
                return Ok(ReadingFreshness::Unavailable);
            };
            let before = Instant::now();
            let result = group.read();
            let after = Instant::now();
            result.map(|reading| (reading, midpoint(before, after)))
        };
        let (reading, sampled_at) = match read_result {
            Ok(sample) => sample,
            Err(_) => {
                state.group = None;
                state.previous = None;
                state.retry = if recovering {
                    if state.last_value.is_some() {
                        RetryPolicy::StaleUntilFullDiscovery
                    } else {
                        RetryPolicy::UnavailableUntilFullDiscovery
                    }
                } else {
                    RetryPolicy::RecoveryNextSample
                };
                return Ok(if state.last_value.is_some() {
                    ReadingFreshness::Stale
                } else {
                    ReadingFreshness::Unavailable
                });
            }
        };

        let previous = state.previous.replace(CounterBaseline {
            reading,
            sampled_at,
        });
        let Some(previous) = previous else {
            return Ok(if recovering && state.last_value.is_some() {
                ReadingFreshness::Stale
            } else {
                ReadingFreshness::Unavailable
            });
        };
        let elapsed = sampled_at.saturating_duration_since(previous.sampled_at);
        let Some(value) = effective_clock(previous.reading, reading, elapsed) else {
            if recovering {
                state.group = None;
                state.previous = None;
                state.retry = if state.last_value.is_some() {
                    RetryPolicy::StaleUntilFullDiscovery
                } else {
                    RetryPolicy::UnavailableUntilFullDiscovery
                };
            }
            return Ok(if state.last_value.is_some() {
                ReadingFreshness::Stale
            } else {
                ReadingFreshness::Unavailable
            });
        };

        state.retry = RetryPolicy::NextSample;
        state.last_value = Some(value);
        state.last_updated_unix_ms = Some(captured_at);
        Ok(ReadingFreshness::Current)
    }

    fn midpoint(before: Instant, after: Instant) -> Instant {
        before
            .checked_add(after.saturating_duration_since(before) / 2)
            .unwrap_or(after)
    }

    fn effective_clock(
        previous: PerfGroupReading,
        current: PerfGroupReading,
        elapsed: Duration,
    ) -> Option<f64> {
        if elapsed.is_zero() || elapsed > MAX_INTERVAL {
            return None;
        }
        let aperf = current.first.checked_sub(previous.first)?;
        let mperf = current.second.checked_sub(previous.second)?;
        let enabled = current.time_enabled.checked_sub(previous.time_enabled)?;
        let running = current.time_running.checked_sub(previous.time_running)?;
        if enabled == 0 || running == 0 || running > enabled || ((aperf == 0) != (mperf == 0)) {
            return None;
        }
        let scaled_aperf = aperf as f64 * enabled as f64 / running as f64;
        let frequency = scaled_aperf / elapsed.as_secs_f64();
        (frequency.is_finite() && (0.0..=MAX_EFFECTIVE_HZ).contains(&frequency))
            .then_some(frequency)
    }

    fn cpu_reading(cpu: u32, state: &CpuState, freshness: ReadingFreshness) -> CpuReading {
        CpuReading {
            cpu,
            value: if freshness == ReadingFreshness::Offline {
                Some(0.0)
            } else {
                state.last_value
            },
            last_updated_unix_ms: state.last_updated_unix_ms,
            freshness,
        }
    }

    fn aggregate_readings(readings: &[CpuReading]) -> AggregateReading {
        let mut sum = 0.0;
        let mut measured = 0;
        let mut included = 0;
        let mut stale = false;
        let mut unavailable = false;

        for reading in readings {
            match reading.freshness {
                ReadingFreshness::Current => {
                    if let Some(value) = reading.value {
                        sum += value;
                        measured += 1;
                        included += 1;
                    } else {
                        unavailable = true;
                    }
                }
                ReadingFreshness::Offline => {
                    included += 1;
                }
                ReadingFreshness::Stale => stale = true,
                ReadingFreshness::Unavailable => unavailable = true,
            }
        }

        if included > 0 {
            AggregateReading {
                value: Some(sum / included as f64),
                freshness: if stale || unavailable {
                    ReadingFreshness::Stale
                } else {
                    ReadingFreshness::Current
                },
                measured,
                included,
            }
        } else {
            AggregateReading {
                value: None,
                freshness: if stale {
                    ReadingFreshness::Stale
                } else {
                    ReadingFreshness::Unavailable
                },
                measured,
                included,
            }
        }
    }

    fn attach_readings(snapshot: &mut Snapshot, readings: &[CpuReading]) {
        let device = cpu_device(snapshot);
        for reading in readings {
            let mut sensor = effective_sensor(
                format!("cpu:0:effective_clock:logical:{}", reading.cpu),
                format!("CPU {} effective clock", reading.cpu),
                reading.value,
                format!(
                    "{SOURCE_PREFIX}/aperf, {SOURCE_PREFIX}/mperf (CPU {})",
                    reading.cpu
                ),
                reading.freshness,
                reading.last_updated_unix_ms,
            );
            sensor
                .metadata
                .insert("logical_cpu".to_owned(), u64::from(reading.cpu).into());
            device.sensors.push(sensor);
        }
    }

    fn effective_sensor(
        id: String,
        label: String,
        value: Option<f64>,
        source: String,
        freshness: ReadingFreshness,
        last_updated_unix_ms: Option<u128>,
    ) -> Sensor {
        let mut sensor = Sensor::new(
            id,
            label,
            SensorKind::EffectiveClock,
            Unit::Hertz,
            value,
            source,
            Identification::Inferred,
        );
        sensor.freshness = freshness;
        sensor.last_updated_unix_ms = last_updated_unix_ms;
        sensor.mark_collector(CollectorId::EffectiveClock);
        sensor
            .metadata
            .insert("backend".to_owned(), "perf_event".into());
        sensor
            .metadata
            .insert("counter_source".to_owned(), "aperf_mperf".into());
        sensor
    }

    fn cpu_device(snapshot: &mut Snapshot) -> &mut Device {
        if let Some(index) = snapshot
            .devices
            .iter()
            .position(|device| device.id == "cpu:0")
        {
            return &mut snapshot.devices[index];
        }
        let index = snapshot.devices.len();
        snapshot
            .devices
            .push(Device::new("cpu:0", DeviceClass::Cpu, "CPU"));
        &mut snapshot.devices[index]
    }

    fn discover_events() -> Option<EventPair> {
        let pmu = Path::new(MSR_PMU);
        Some(EventPair {
            aperf: RawPerfEvent::discover(pmu, "aperf").ok()?,
            mperf: RawPerfEvent::discover(pmu, "mperf").ok()?,
        })
    }

    fn read_topology() -> io::Result<CpuTopology> {
        let present = parse_cpu_list(&read_small_trimmed(Path::new(CPU_PRESENT))?)?;
        let online = parse_cpu_list(&read_small_trimmed(Path::new(CPU_ONLINE))?)?;
        if present.is_empty() || !online.is_subset(&present) {
            return Err(invalid_data("invalid CPU present/online masks"));
        }
        Ok(CpuTopology { present, online })
    }

    fn descriptor_budget_allows(online_cpus: usize, open_groups: usize) -> bool {
        let Some(soft_limit) = soft_file_descriptor_limit() else {
            return true;
        };
        let Ok(open_descriptors) = fs::read_dir(PROC_SELF_FD) else {
            return true;
        };
        let open_descriptors = open_descriptors.filter_map(Result::ok).count() as u128;
        descriptor_budget_allows_values(
            soft_limit,
            open_descriptors,
            open_groups as u128,
            online_cpus as u128,
        )
    }

    fn descriptor_budget_allows_values(
        soft_limit: u128,
        open_descriptors: u128,
        open_groups: u128,
        online_cpus: u128,
    ) -> bool {
        let effective_descriptors = open_groups.saturating_mul(PERF_FDS_PER_CPU);
        let other_descriptors = open_descriptors.saturating_sub(effective_descriptors);
        let required_effective_descriptors = online_cpus.saturating_mul(PERF_FDS_PER_CPU);
        other_descriptors
            .saturating_add(FD_HEADROOM)
            .saturating_add(required_effective_descriptors)
            <= soft_limit
    }

    fn soft_file_descriptor_limit() -> Option<u128> {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: `limit` points to valid writable storage for getrlimit(2).
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0
            || limit.rlim_cur == libc::RLIM_INFINITY
        {
            None
        } else {
            Some(limit.rlim_cur as u128)
        }
    }

    fn is_global_open_failure(error: &io::Error) -> bool {
        matches!(
            error.raw_os_error(),
            Some(code)
                if code == libc::EMFILE
                    || code == libc::ENFILE
                    || code == libc::EACCES
                    || code == libc::EPERM
                    || code == libc::ENOSYS
                    || code == libc::EOPNOTSUPP
        )
    }

    fn read_small_trimmed(path: &Path) -> io::Result<String> {
        let mut bytes = Vec::new();
        File::open(path)?
            .take(MAX_SYSFS_VALUE_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_SYSFS_VALUE_BYTES {
            return Err(invalid_data("CPU mask is unexpectedly large"));
        }
        let value = String::from_utf8(bytes).map_err(|_| invalid_data("CPU mask is not UTF-8"))?;
        let value = value.trim();
        if value.is_empty() {
            Err(invalid_data("CPU mask is empty"))
        } else {
            Ok(value.to_owned())
        }
    }

    fn parse_cpu_list(value: &str) -> io::Result<BTreeSet<u32>> {
        let mut cpus = BTreeSet::new();
        for token in value.split(',').map(str::trim) {
            if token.is_empty() {
                return Err(invalid_data("empty CPU-list token"));
            }
            let (start, end) = if let Some((start, end)) = token.split_once('-') {
                (parse_cpu_id(start)?, parse_cpu_id(end)?)
            } else {
                let cpu = parse_cpu_id(token)?;
                (cpu, cpu)
            };
            if start > end {
                return Err(invalid_data("reversed CPU range"));
            }
            let range_count = u64::from(end) - u64::from(start) + 1;
            if range_count > MAX_CPU_COUNT as u64 {
                return Err(invalid_data("CPU list exceeds the supported limit"));
            }
            cpus.extend(start..=end);
            if cpus.len() > MAX_CPU_COUNT {
                return Err(invalid_data("CPU list exceeds the supported limit"));
            }
        }
        Ok(cpus)
    }

    fn parse_cpu_id(value: &str) -> io::Result<u32> {
        let cpu = value
            .trim()
            .parse::<u32>()
            .map_err(|_| invalid_data("invalid CPU ID"))?;
        if cpu > MAX_CPU_ID {
            Err(invalid_data("CPU ID exceeds the supported limit"))
        } else {
            Ok(cpu)
        }
    }

    fn invalid_data(message: &'static str) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, message)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn reading(aperf: u64, mperf: u64, enabled: u64, running: u64) -> PerfGroupReading {
            PerfGroupReading {
                first: aperf,
                second: mperf,
                time_enabled: enabled,
                time_running: running,
            }
        }

        fn cpu_reading_with(value: Option<f64>, freshness: ReadingFreshness) -> CpuReading {
            CpuReading {
                cpu: 0,
                value,
                last_updated_unix_ms: None,
                freshness,
            }
        }

        #[test]
        fn parses_sparse_cpu_masks() {
            assert_eq!(
                parse_cpu_list("0-3,8,16-17").unwrap(),
                BTreeSet::from([0, 1, 2, 3, 8, 16, 17])
            );
        }

        #[test]
        fn rejects_unsafe_cpu_masks_without_expanding_them() {
            assert!(parse_cpu_list("7-3").is_err());
            assert!(parse_cpu_list("0-4294967295").is_err());
            assert!(parse_cpu_list("0,,2").is_err());
        }

        #[test]
        fn calculates_idle_inclusive_effective_clock() {
            let frequency = effective_clock(
                reading(100, 100, 1_000, 1_000),
                reading(41_251_285, 56_984_122, 1_001_000, 1_001_000),
                Duration::from_secs(1),
            )
            .unwrap();
            assert!((frequency - 41_251_185.0).abs() < 0.1);
        }

        #[test]
        fn scales_multiplexed_counts() {
            let frequency = effective_clock(
                reading(0, 0, 0, 0),
                reading(100_000_000, 100_000_000, 1_000, 500),
                Duration::from_secs(1),
            )
            .unwrap();
            assert_eq!(frequency, 200_000_000.0);
        }

        #[test]
        fn accepts_fully_idle_intervals() {
            assert_eq!(
                effective_clock(
                    reading(10, 10, 10, 10),
                    reading(10, 10, 20, 20),
                    Duration::from_secs(1),
                ),
                Some(0.0)
            );
        }

        #[test]
        fn rejects_regressions_asymmetric_counters_and_impossible_results() {
            assert!(
                effective_clock(
                    reading(100, 100, 100, 100),
                    reading(99, 101, 200, 200),
                    Duration::from_secs(1),
                )
                .is_none()
            );
            assert!(
                effective_clock(
                    reading(100, 100, 100, 100),
                    reading(100, 101, 200, 200),
                    Duration::from_secs(1),
                )
                .is_none()
            );
            assert!(
                effective_clock(
                    reading(0, 0, 0, 0),
                    reading(200_000_000_000, 200_000_000_000, 1, 1),
                    Duration::from_secs(1),
                )
                .is_none()
            );
        }

        #[test]
        fn reserves_descriptor_headroom_for_the_complete_cpu_set() {
            assert!(descriptor_budget_allows_values(1_024, 64, 16, 400));
            assert!(!descriptor_budget_allows_values(1_024, 64, 16, 450));
        }

        #[test]
        fn existing_effective_descriptors_are_not_counted_twice() {
            assert!(descriptor_budget_allows_values(1_024, 800, 400, 448));
        }

        #[test]
        fn offline_cpus_are_zero_and_count_toward_the_average() {
            let readings = [
                cpu_reading_with(Some(2_000_000_000.0), ReadingFreshness::Current),
                cpu_reading_with(Some(0.0), ReadingFreshness::Offline),
            ];
            assert_eq!(
                aggregate_readings(&readings),
                AggregateReading {
                    value: Some(1_000_000_000.0),
                    freshness: ReadingFreshness::Current,
                    measured: 1,
                    included: 2,
                }
            );
        }

        #[test]
        fn partial_average_excludes_unknown_cpus_and_is_stale() {
            let readings = [
                cpu_reading_with(Some(2_000_000_000.0), ReadingFreshness::Current),
                cpu_reading_with(Some(0.0), ReadingFreshness::Offline),
                cpu_reading_with(Some(4_000_000_000.0), ReadingFreshness::Unavailable),
            ];
            assert_eq!(
                aggregate_readings(&readings),
                AggregateReading {
                    value: Some(1_000_000_000.0),
                    freshness: ReadingFreshness::Stale,
                    measured: 1,
                    included: 2,
                }
            );
        }

        #[test]
        fn all_offline_cpus_produce_a_current_zero_average() {
            let readings = [
                cpu_reading_with(Some(0.0), ReadingFreshness::Offline),
                cpu_reading_with(Some(0.0), ReadingFreshness::Offline),
            ];
            assert_eq!(
                aggregate_readings(&readings),
                AggregateReading {
                    value: Some(0.0),
                    freshness: ReadingFreshness::Current,
                    measured: 0,
                    included: 2,
                }
            );
        }

        #[test]
        fn recovery_wait_states_preserve_their_freshness() {
            assert_eq!(
                RetryPolicy::StaleUntilFullDiscovery.waiting_freshness(),
                Some(ReadingFreshness::Stale)
            );
            assert_eq!(
                RetryPolicy::UnavailableUntilFullDiscovery.waiting_freshness(),
                Some(ReadingFreshness::Unavailable)
            );
            assert_eq!(RetryPolicy::RecoveryNextSample.waiting_freshness(), None);
            assert_eq!(
                RetryPolicy::StaleUntilFullDiscovery.after_full_discovery(),
                RetryPolicy::RecoveryNextSample
            );
            assert_eq!(
                RetryPolicy::UnavailableUntilFullDiscovery.after_full_discovery(),
                RetryPolicy::NextSample
            );
        }

        #[test]
        fn midpoint_tracks_each_individual_counter_read() {
            let before = Instant::now();
            let after = before + Duration::from_millis(10);
            assert_eq!(midpoint(before, after), before + Duration::from_millis(5));
        }
    }
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
mod platform {
    use crate::model::Snapshot;

    #[derive(Debug, Default)]
    pub(crate) struct EffectiveClockCollector;

    impl EffectiveClockCollector {
        pub(crate) fn new() -> Self {
            Self
        }

        pub(crate) fn sample(&mut self, _snapshot: &mut Snapshot, _full_discovery: bool) {}
    }
}

pub(crate) use platform::EffectiveClockCollector;
