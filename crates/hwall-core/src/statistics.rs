//! Running statistics for live telemetry consumers.
//!
//! Statistics are keyed by stable device and sensor identifiers. Hardware
//! thresholds (`Sensor::min`, `Sensor::max`, and `Sensor::critical`) remain
//! separate from values observed during the current monitoring session.

use crate::{Sensor, SensorKind, SensorStatus, Snapshot, Unit};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RunningStatistics {
    pub samples: u64,
    pub minimum: f64,
    pub maximum: f64,
    pub average: f64,
    pub kind: SensorKind,
    pub unit: Unit,
}

impl RunningStatistics {
    fn first(sensor: &Sensor, value: f64) -> Self {
        Self {
            samples: 1,
            minimum: value,
            maximum: value,
            average: value,
            kind: sensor.kind,
            unit: sensor.unit,
        }
    }

    fn observe(&mut self, sensor: &Sensor, value: f64) {
        if self.kind != sensor.kind || self.unit != sensor.unit {
            *self = Self::first(sensor, value);
            return;
        }

        self.samples = self.samples.saturating_add(1);
        self.minimum = self.minimum.min(value);
        self.maximum = self.maximum.max(value);
        self.average += (value - self.average) / self.samples as f64;
    }
}

#[derive(Debug, Clone, Default)]
pub struct SnapshotStatistics {
    entries: BTreeMap<String, BTreeMap<String, RunningStatistics>>,
    sample_rounds: u64,
}

impl SnapshotStatistics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Observe all valid numeric readings in a snapshot.
    ///
    /// Faulted, unavailable, and non-finite values are ignored. Alarm readings
    /// are retained because the value itself is still useful telemetry.
    pub fn observe(&mut self, snapshot: &Snapshot) {
        self.sample_rounds = self.sample_rounds.saturating_add(1);
        for device in &snapshot.devices {
            for sensor in &device.sensors {
                if sensor.is_intermittent() {
                    continue;
                }
                if matches!(
                    sensor.status,
                    SensorStatus::Fault | SensorStatus::Unavailable
                ) {
                    continue;
                }
                let Some(value) = sensor.value.filter(|value| value.is_finite()) else {
                    continue;
                };
                let device_statistics = self.entries.entry(device.id.clone()).or_default();
                match device_statistics.get_mut(&sensor.id) {
                    Some(statistics) => statistics.observe(sensor, value),
                    None => {
                        device_statistics
                            .insert(sensor.id.clone(), RunningStatistics::first(sensor, value));
                    }
                }
            }
        }
    }

    pub fn get(&self, device_id: &str, sensor_id: &str) -> Option<&RunningStatistics> {
        self.entries
            .get(device_id)
            .and_then(|device| device.get(sensor_id))
    }

    pub fn sample_rounds(&self) -> u64 {
        self.sample_rounds
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.sample_rounds = 0;
    }

    /// Reset the monitoring window and immediately seed it with the current
    /// snapshot, so all four columns have a useful value after reset.
    pub fn reset_with(&mut self, snapshot: &Snapshot) {
        self.clear();
        self.observe(snapshot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Device, DeviceClass, Identification};

    fn snapshot_with(value: f64) -> Snapshot {
        let mut snapshot = Snapshot::new();
        let mut device = Device::new("cpu:0", DeviceClass::Cpu, "CPU");
        device.sensors.push(Sensor::new(
            "temp:package",
            "Package",
            SensorKind::Temperature,
            Unit::Celsius,
            Some(value),
            "/sys/example",
            Identification::KernelLabel,
        ));
        snapshot.devices.push(device);
        snapshot
    }

    #[test]
    fn calculates_minimum_maximum_and_incremental_average() {
        let mut statistics = SnapshotStatistics::new();
        statistics.observe(&snapshot_with(40.0));
        statistics.observe(&snapshot_with(50.0));
        statistics.observe(&snapshot_with(45.0));

        let value = statistics
            .get("cpu:0", "temp:package")
            .expect("tracked sensor");
        assert_eq!(value.samples, 3);
        assert_eq!(value.minimum, 40.0);
        assert_eq!(value.maximum, 50.0);
        assert_eq!(value.average, 45.0);
    }

    #[test]
    fn reset_seeds_statistics_with_current_snapshot() {
        let mut statistics = SnapshotStatistics::new();
        statistics.observe(&snapshot_with(40.0));
        statistics.observe(&snapshot_with(50.0));
        statistics.reset_with(&snapshot_with(47.0));

        let value = statistics
            .get("cpu:0", "temp:package")
            .expect("tracked sensor");
        assert_eq!(statistics.sample_rounds(), 1);
        assert_eq!(value.samples, 1);
        assert_eq!(value.minimum, 47.0);
        assert_eq!(value.maximum, 47.0);
        assert_eq!(value.average, 47.0);
    }

    #[test]
    fn ignores_unavailable_readings() {
        let mut unavailable = snapshot_with(99.0);
        unavailable.devices[0].sensors[0].status = SensorStatus::Unavailable;

        let mut statistics = SnapshotStatistics::new();
        statistics.observe(&snapshot_with(40.0));
        statistics.observe(&unavailable);

        let value = statistics
            .get("cpu:0", "temp:package")
            .expect("tracked sensor");
        assert_eq!(value.samples, 1);
        assert_eq!(value.maximum, 40.0);
    }
}
