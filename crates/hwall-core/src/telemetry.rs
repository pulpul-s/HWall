//! Stateful derivation of rates from cumulative kernel counters.
//!
//! Collectors only report raw hardware state. This layer owns elapsed-time,
//! reset, and suspend-gap handling so CPU, network, storage, and energy-derived
//! power use identical semantics.

use crate::{Device, Identification, Sensor, SensorKind, Snapshot, Unit};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

const DERIVED_BY_KEY: &str = "derived_by";
const DERIVED_BY_VALUE: &str = "hwall-telemetry";
const MAX_RATE_INTERVAL: Duration = Duration::from_secs(300);

#[derive(Debug, Default)]
pub(crate) struct TelemetryDeriver {
    previous: BTreeMap<String, BTreeMap<String, u64>>,
    previous_at: Option<Instant>,
}

impl TelemetryDeriver {
    pub(crate) fn apply(&mut self, mut snapshot: Snapshot) -> Snapshot {
        self.apply_at(&mut snapshot, Instant::now());
        snapshot
    }

    fn apply_at(&mut self, snapshot: &mut Snapshot, now: Instant) {
        for device in &mut snapshot.devices {
            device
                .sensors
                .retain(|sensor| sensor.metadata_str(DERIVED_BY_KEY) != Some(DERIVED_BY_VALUE));
        }

        let elapsed = self
            .previous_at
            .map(|previous| now.saturating_duration_since(previous));
        let valid_interval =
            elapsed.filter(|elapsed| !elapsed.is_zero() && *elapsed <= MAX_RATE_INTERVAL);

        if let Some(elapsed) = valid_interval {
            let seconds = elapsed.as_secs_f64();
            for device in &mut snapshot.devices {
                let Some(previous) = self.previous.get(&device.id) else {
                    continue;
                };
                derive_cpu(device, previous);
                derive_network(device, previous, seconds);
                derive_storage(device, previous, elapsed);
                derive_hwmon_power(device, previous, seconds);
            }
        }

        self.previous = snapshot
            .devices
            .iter()
            .filter(|device| !device.counters.is_empty())
            .map(|device| (device.id.clone(), device.counters.clone()))
            .collect();
        self.previous_at = Some(now);
        snapshot.sort();
    }
}

fn derive_cpu(device: &mut Device, previous: &BTreeMap<String, u64>) {
    if let Some(utilization) =
        cpu_utilization(device, previous, "cpu_total_ticks", "cpu_idle_ticks")
    {
        push_derived_sensor(
            device,
            "cpu:0:utilization:total",
            "Total CPU utilization",
            SensorKind::Utilization,
            Unit::Percent,
            utilization,
            "/proc/stat",
        );
    }

    let mut logical_cpus = device
        .counters
        .keys()
        .filter_map(|key| {
            key.strip_prefix("cpu_logical_")?
                .strip_suffix("_total_ticks")?
                .parse::<u32>()
                .ok()
        })
        .collect::<Vec<_>>();
    logical_cpus.sort_unstable();
    logical_cpus.dedup();

    for cpu in logical_cpus {
        let total_key = format!("cpu_logical_{cpu}_total_ticks");
        let idle_key = format!("cpu_logical_{cpu}_idle_ticks");
        let Some(utilization) = cpu_utilization(device, previous, &total_key, &idle_key) else {
            continue;
        };
        push_derived_sensor(
            device,
            format!("cpu:0:utilization:logical:{cpu}"),
            format!("CPU {cpu} utilization"),
            SensorKind::Utilization,
            Unit::Percent,
            utilization,
            "/proc/stat",
        );
    }
}

fn cpu_utilization(
    device: &Device,
    previous: &BTreeMap<String, u64>,
    total_key: &str,
    idle_key: &str,
) -> Option<f64> {
    let total_delta = counter_delta(device, previous, total_key)?;
    let idle_delta = counter_delta(device, previous, idle_key)?;
    if total_delta == 0 || idle_delta > total_delta {
        return None;
    }
    Some(((total_delta - idle_delta) as f64 * 100.0 / total_delta as f64).clamp(0.0, 100.0))
}

fn derive_network(device: &mut Device, previous: &BTreeMap<String, u64>, seconds: f64) {
    if !device.id.starts_with("net:") || seconds <= 0.0 {
        return;
    }
    for (key, id, label, unit) in [
        (
            "rx_bytes",
            "receive_bytes",
            "Receive rate",
            Unit::BytePerSecond,
        ),
        (
            "tx_bytes",
            "transmit_bytes",
            "Transmit rate",
            Unit::BytePerSecond,
        ),
        (
            "rx_packets",
            "receive_packets",
            "Receive packet rate",
            Unit::CountPerSecond,
        ),
        (
            "tx_packets",
            "transmit_packets",
            "Transmit packet rate",
            Unit::CountPerSecond,
        ),
    ] {
        let Some(delta) = counter_delta(device, previous, key) else {
            continue;
        };
        push_derived_sensor(
            device,
            format!("{}:rate:{id}", device.id),
            label,
            SensorKind::Throughput,
            unit,
            delta as f64 / seconds,
            format!("/sys/class/net/{}/statistics/{key}", network_name(device)),
        );
    }
}

fn derive_storage(device: &mut Device, previous: &BTreeMap<String, u64>, elapsed: Duration) {
    if !device.id.starts_with("block:") || !device.counters.contains_key("read_sectors") {
        return;
    }
    let seconds = elapsed.as_secs_f64();
    if seconds <= 0.0 {
        return;
    }
    let name = device
        .bus_address
        .as_deref()
        .or_else(|| device.id.strip_prefix("block:"))
        .unwrap_or(device.id.as_str());
    let source = format!("/sys/class/block/{name}/stat");
    for (key, id, label, scale, unit) in [
        (
            "read_sectors",
            "read_bytes",
            "Read throughput",
            512.0,
            Unit::BytePerSecond,
        ),
        (
            "write_sectors",
            "write_bytes",
            "Write throughput",
            512.0,
            Unit::BytePerSecond,
        ),
        (
            "read_operations",
            "read_operations",
            "Read operations",
            1.0,
            Unit::CountPerSecond,
        ),
        (
            "write_operations",
            "write_operations",
            "Write operations",
            1.0,
            Unit::CountPerSecond,
        ),
    ] {
        let Some(delta) = counter_delta(device, previous, key) else {
            continue;
        };
        push_derived_sensor(
            device,
            format!("{}:activity:{id}", device.id),
            label,
            SensorKind::Throughput,
            unit,
            delta as f64 * scale / seconds,
            source.clone(),
        );
    }

    if let Some(io_ms) = counter_delta(device, previous, "io_milliseconds") {
        let elapsed_ms = elapsed.as_secs_f64() * 1_000.0;
        if elapsed_ms > 0.0 {
            push_derived_sensor(
                device,
                format!("{}:activity:busy", device.id),
                "Device utilization",
                SensorKind::Utilization,
                Unit::Percent,
                (io_ms as f64 * 100.0 / elapsed_ms).clamp(0.0, 100.0),
                source,
            );
        }
    }
}

fn derive_hwmon_power(device: &mut Device, previous: &BTreeMap<String, u64>, seconds: f64) {
    if seconds <= 0.0 {
        return;
    }

    let derived = device
        .sensors
        .iter()
        .filter(|sensor| sensor.kind == SensorKind::Energy)
        .filter_map(|sensor| {
            let counter_key = sensor.metadata_str("energy_counter_key")?;
            if has_direct_hwmon_power(&device.sensors, sensor) {
                return None;
            }
            let delta = counter_delta(device, previous, counter_key)?;
            Some((
                format!("{}:average-power", sensor.id),
                power_label(&sensor.label),
                delta as f64 / 1_000_000.0 / seconds,
                sensor.source.clone(),
                sensor.id.clone(),
            ))
        })
        .collect::<Vec<_>>();

    for (id, label, value, source, energy_sensor_id) in derived {
        let mut sensor =
            new_derived_sensor(id, label, SensorKind::Power, Unit::Watt, value, source);
        sensor
            .metadata
            .insert("derived_from".to_owned(), energy_sensor_id.into());
        sensor
            .metadata
            .insert("aggregation".to_owned(), "interval_average".into());
        device.sensors.push(sensor);
    }
}

fn has_direct_hwmon_power(sensors: &[Sensor], energy: &Sensor) -> bool {
    let Some(hwmon_name) = energy.metadata_str("hwmon_name") else {
        return false;
    };
    let Some(channel) = energy.metadata_u64("hwmon_channel") else {
        return false;
    };
    sensors.iter().any(|sensor| {
        sensor.kind == SensorKind::Power
            && sensor.metadata_str("hwmon_name") == Some(hwmon_name)
            && sensor.metadata_u64("hwmon_channel") == Some(channel)
    })
}

fn power_label(label: &str) -> String {
    if let Some(number) = label.strip_prefix("Energy ") {
        format!("Power {number}")
    } else if let Some(base) = label.strip_suffix(" energy") {
        format!("{base} power")
    } else {
        format!("{label} power")
    }
}

fn counter_delta(device: &Device, previous: &BTreeMap<String, u64>, key: &str) -> Option<u64> {
    let current = *device.counters.get(key)?;
    let previous = *previous.get(key)?;
    current.checked_sub(previous)
}

fn push_derived_sensor(
    device: &mut Device,
    id: impl Into<String>,
    label: impl Into<String>,
    kind: SensorKind,
    unit: Unit,
    value: f64,
    source: impl Into<String>,
) {
    if value.is_finite() {
        device
            .sensors
            .push(new_derived_sensor(id, label, kind, unit, value, source));
    }
}

fn new_derived_sensor(
    id: impl Into<String>,
    label: impl Into<String>,
    kind: SensorKind,
    unit: Unit,
    value: f64,
    source: impl Into<String>,
) -> Sensor {
    let mut sensor = Sensor::new(
        id,
        label,
        kind,
        unit,
        Some(value),
        source,
        Identification::Inferred,
    );
    sensor
        .metadata
        .insert(DERIVED_BY_KEY.to_owned(), DERIVED_BY_VALUE.into());
    sensor
}

fn network_name(device: &Device) -> &str {
    device.id.strip_prefix("net:").unwrap_or(device.id.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DeviceClass, PropertyValue, Snapshot};

    fn snapshot_with_counters(id: &str, class: DeviceClass, values: &[(&str, u64)]) -> Snapshot {
        let mut snapshot = Snapshot::new();
        let mut device = Device::new(id, class, id);
        device.counters.extend(
            values
                .iter()
                .map(|(key, value)| ((*key).to_owned(), *value)),
        );
        snapshot.devices.push(device);
        snapshot
    }

    fn snapshot_with_hwmon_energy(value_uj: u64, direct_power: bool) -> Snapshot {
        let mut snapshot = Snapshot::new();
        let mut device = Device::new("cpu:0", DeviceClass::Cpu, "CPU");
        let energy_id = "cpu:0:hwmon:amd_energy:energy1_input";
        let counter_key = format!("hwmon_energy_uj:{energy_id}");
        let mut energy = Sensor::new(
            energy_id,
            "CPU package",
            SensorKind::Energy,
            Unit::Joule,
            Some(value_uj as f64 / 1_000_000.0),
            "/sys/class/hwmon/hwmon0/energy1_input",
            Identification::KernelLabel,
        );
        energy
            .metadata
            .insert("hwmon_name".to_owned(), "amd_energy".into());
        energy
            .metadata
            .insert("hwmon_channel".to_owned(), 1_u64.into());
        energy
            .metadata
            .insert("energy_counter_key".to_owned(), counter_key.clone().into());
        device.counters.insert(counter_key, value_uj);
        device.sensors.push(energy);

        if direct_power {
            let mut power = Sensor::new(
                "cpu:0:hwmon:amd_energy:power1_input",
                "CPU package power",
                SensorKind::Power,
                Unit::Watt,
                Some(12.0),
                "/sys/class/hwmon/hwmon0/power1_input",
                Identification::KernelLabel,
            );
            power
                .metadata
                .insert("hwmon_name".to_owned(), "amd_energy".into());
            power
                .metadata
                .insert("hwmon_channel".to_owned(), 1_u64.into());
            device.sensors.push(power);
        }

        snapshot.devices.push(device);
        snapshot
    }

    fn derived_power(snapshot: &Snapshot) -> Option<&Sensor> {
        snapshot.devices[0].sensors.iter().find(|sensor| {
            sensor.kind == SensorKind::Power
                && matches!(
                    sensor.metadata.get(DERIVED_BY_KEY),
                    Some(PropertyValue::String(value)) if value == DERIVED_BY_VALUE
                )
        })
    }

    #[test]
    fn derives_cpu_utilization_after_second_sample() {
        let start = Instant::now();
        let mut deriver = TelemetryDeriver::default();
        let mut first = snapshot_with_counters(
            "cpu:0",
            DeviceClass::Cpu,
            &[("cpu_total_ticks", 100), ("cpu_idle_ticks", 80)],
        );
        deriver.apply_at(&mut first, start);
        assert!(first.devices[0].sensors.is_empty());

        let mut second = snapshot_with_counters(
            "cpu:0",
            DeviceClass::Cpu,
            &[("cpu_total_ticks", 200), ("cpu_idle_ticks", 130)],
        );
        deriver.apply_at(&mut second, start + Duration::from_secs(1));
        assert_eq!(second.devices[0].sensors[0].value, Some(50.0));
    }

    #[test]
    fn derives_logical_cpu_utilization() {
        let start = Instant::now();
        let mut deriver = TelemetryDeriver::default();
        let counters = [
            ("cpu_total_ticks", 200),
            ("cpu_idle_ticks", 150),
            ("cpu_logical_0_total_ticks", 100),
            ("cpu_logical_0_idle_ticks", 80),
            ("cpu_logical_1_total_ticks", 100),
            ("cpu_logical_1_idle_ticks", 70),
        ];
        let mut first = snapshot_with_counters("cpu:0", DeviceClass::Cpu, &counters);
        deriver.apply_at(&mut first, start);

        let counters = [
            ("cpu_total_ticks", 400),
            ("cpu_idle_ticks", 250),
            ("cpu_logical_0_total_ticks", 200),
            ("cpu_logical_0_idle_ticks", 130),
            ("cpu_logical_1_total_ticks", 200),
            ("cpu_logical_1_idle_ticks", 80),
        ];
        let mut second = snapshot_with_counters("cpu:0", DeviceClass::Cpu, &counters);
        deriver.apply_at(&mut second, start + Duration::from_secs(1));

        let sensor_value = |id: &str| {
            second.devices[0]
                .sensors
                .iter()
                .find(|sensor| sensor.id == id)
                .and_then(|sensor| sensor.value)
        };
        assert_eq!(sensor_value("cpu:0:utilization:total"), Some(50.0));
        assert_eq!(sensor_value("cpu:0:utilization:logical:0"), Some(50.0));
        assert_eq!(sensor_value("cpu:0:utilization:logical:1"), Some(90.0));
    }

    #[test]
    fn counter_reset_does_not_create_a_rate_spike() {
        let start = Instant::now();
        let mut deriver = TelemetryDeriver::default();
        let mut first =
            snapshot_with_counters("net:eth0", DeviceClass::Network, &[("rx_bytes", 1000)]);
        deriver.apply_at(&mut first, start);
        let mut reset =
            snapshot_with_counters("net:eth0", DeviceClass::Network, &[("rx_bytes", 10)]);
        deriver.apply_at(&mut reset, start + Duration::from_secs(1));
        assert!(reset.devices[0].sensors.is_empty());
    }

    #[test]
    fn missing_counter_sample_reseeds_the_rate() {
        let start = Instant::now();
        let mut deriver = TelemetryDeriver::default();
        let mut first =
            snapshot_with_counters("net:eth0", DeviceClass::Network, &[("rx_bytes", 1000)]);
        deriver.apply_at(&mut first, start);

        let mut missing = Snapshot::new();
        deriver.apply_at(&mut missing, start + Duration::from_secs(1));

        let mut reappeared =
            snapshot_with_counters("net:eth0", DeviceClass::Network, &[("rx_bytes", 2000)]);
        deriver.apply_at(&mut reappeared, start + Duration::from_secs(2));
        assert!(reappeared.devices[0].sensors.is_empty());
    }

    #[test]
    fn derives_average_power_from_hwmon_energy() {
        let start = Instant::now();
        let mut deriver = TelemetryDeriver::default();
        let mut first = snapshot_with_hwmon_energy(1_000_000, false);
        deriver.apply_at(&mut first, start);
        assert!(derived_power(&first).is_none());

        let mut second = snapshot_with_hwmon_energy(3_000_000, false);
        deriver.apply_at(&mut second, start + Duration::from_secs(2));
        let power = derived_power(&second).expect("derived power sensor");
        assert_eq!(power.label, "CPU package power");
        assert_eq!(power.value, Some(1.0));
        assert_eq!(power.unit, Unit::Watt);
    }

    #[test]
    fn direct_hwmon_power_suppresses_energy_derived_power() {
        let start = Instant::now();
        let mut deriver = TelemetryDeriver::default();
        let mut first = snapshot_with_hwmon_energy(1_000_000, true);
        deriver.apply_at(&mut first, start);

        let mut second = snapshot_with_hwmon_energy(3_000_000, true);
        deriver.apply_at(&mut second, start + Duration::from_secs(1));
        assert!(derived_power(&second).is_none());
        assert_eq!(
            second.devices[0]
                .sensors
                .iter()
                .filter(|sensor| sensor.kind == SensorKind::Power)
                .count(),
            1
        );
    }

    #[test]
    fn hwmon_energy_reset_skips_one_power_sample() {
        let start = Instant::now();
        let mut deriver = TelemetryDeriver::default();
        let mut first = snapshot_with_hwmon_energy(3_000_000, false);
        deriver.apply_at(&mut first, start);

        let mut reset = snapshot_with_hwmon_energy(1_000_000, false);
        deriver.apply_at(&mut reset, start + Duration::from_secs(1));
        assert!(derived_power(&reset).is_none());
    }
}
