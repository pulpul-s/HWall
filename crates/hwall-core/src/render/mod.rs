//! Human and diagnostic rendering for [`crate::Snapshot`].
//!
//! The default hardware view is complete but de-duplicated: it presents every
//! useful physical device and available reading under its owner. Diagnostic mode
//! adds transport nodes, raw identifiers, source paths, and collector internals.

mod diagnostic;
mod format;
mod hardware;

pub use format::{
    escape_delimited, format_sample_age, format_sample_age_compact, format_value,
    hardware_property_label, humanize_key, is_low_level_hardware_property, property_to_string,
    sensor_kind_name, storage_health_property_label,
};
pub use hardware::{format_property_value, hardware_device_visible};

use crate::{Snapshot, SnapshotStatistics};

/// Render a terminal report.
///
/// `diagnostic = false` produces the normal full hardware view. `true` produces
/// the exhaustive collector/debug representation used by `--verbose`.
pub fn human(snapshot: &Snapshot, diagnostic: bool) -> String {
    if diagnostic {
        diagnostic::render(snapshot, None)
    } else {
        hardware::render(snapshot, None)
    }
}

/// Render a live report with observed minimum, maximum, and average values.
///
/// Hardware thresholds remain separate from session statistics. The statistics
/// window is controlled by the caller and can be reset without recollecting the
/// hardware inventory.
pub fn live(snapshot: &Snapshot, statistics: &SnapshotStatistics, diagnostic: bool) -> String {
    if diagnostic {
        diagnostic::render(snapshot, Some(statistics))
    } else {
        hardware::render(snapshot, Some(statistics))
    }
}

#[cfg(test)]
mod tests {
    use super::live;
    use crate::{
        Device, DeviceClass, Identification, Sensor, SensorKind, Snapshot, SnapshotStatistics, Unit,
    };

    fn snapshot_with_temperature(value: f64) -> Snapshot {
        let mut snapshot = Snapshot::new();
        let mut device = Device::new("gpu:0", DeviceClass::Gpu, "Test GPU");
        device.sensors.push(Sensor::new(
            "temperature:edge",
            "GPU temperature",
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
    fn live_renderer_shows_observed_columns() {
        let mut statistics = SnapshotStatistics::new();
        statistics.observe(&snapshot_with_temperature(40.0));
        let current = snapshot_with_temperature(50.0);
        statistics.observe(&current);

        let report = live(&current, &statistics, false);
        assert!(report.contains("Current"));
        assert!(report.contains("Minimum"));
        assert!(report.contains("Maximum"));
        assert!(report.contains("Average"));
        assert!(report.contains("40.0 °C"));
        assert!(report.contains("50.0 °C"));
        assert!(report.contains("45.0 °C"));
    }
}
