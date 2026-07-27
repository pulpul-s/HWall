use super::format::{
    format_bytes, format_value, humanize_key, numeric_value, property_to_string,
    sensor_status_suffix,
};
use crate::{Sensor, Snapshot, SnapshotStatistics};
use std::fmt::Write;

pub(super) fn render(snapshot: &Snapshot, statistics: Option<&SnapshotStatistics>) -> String {
    let mut out = String::new();
    for device in &snapshot.devices {
        let _ = writeln!(out, "{}", device.name);
        let _ = writeln!(out, "{}", "=".repeat(72));
        if let Some(vendor) = &device.vendor {
            property_line(&mut out, "Vendor", vendor);
        }
        if let Some(model) = &device.model {
            property_line(&mut out, "Model", model);
        }
        if let Some(driver) = &device.driver {
            property_line(&mut out, "Driver", driver);
        }
        if let Some(address) = &device.bus_address {
            property_line(&mut out, "Bus address", address);
        }
        if let Some(parent) = &device.parent {
            property_line(&mut out, "Parent", parent);
        }
        for (key, value) in &device.properties {
            let rendered = if key.ends_with("_bytes") {
                numeric_value(value)
                    .map(format_bytes)
                    .unwrap_or_else(|| property_to_string(value))
            } else {
                property_to_string(value)
            };
            property_line(&mut out, &humanize_key(key), &rendered);
        }
        if !device.sensors.is_empty() && !device.properties.is_empty() {
            let _ = writeln!(out, "{}", "-".repeat(72));
        }
        for sensor in &device.sensors {
            detailed_sensor_line(&mut out, &device.id, sensor, statistics);
        }
        let _ = writeln!(out, "  id: {}", device.id);
        let _ = writeln!(out);
    }

    if !snapshot.warnings.is_empty() {
        let _ = writeln!(out, "Warnings");
        let _ = writeln!(out, "{}", "=".repeat(72));
        for warning in &snapshot.warnings {
            let _ = writeln!(out, "- {warning}");
        }
    }
    out
}

fn property_line(out: &mut String, label: &str, value: &str) {
    let _ = writeln!(out, "{label:<42} {value:>28}");
}

fn detailed_sensor_line(
    out: &mut String,
    device_id: &str,
    sensor: &Sensor,
    statistics: Option<&SnapshotStatistics>,
) {
    let value = match sensor.value {
        Some(value) => format_value(value, &sensor.unit),
        None => sensor
            .raw_value
            .clone()
            .unwrap_or_else(|| "unavailable".to_owned()),
    };
    let suffix = sensor_status_suffix(sensor);
    let _ = writeln!(out, "{:<42} {:>28}{}", sensor.label, value, suffix);
    if let Some(observed) = statistics.and_then(|values| values.get(device_id, &sensor.id)) {
        let _ = writeln!(
            out,
            "  observed: minimum {} • maximum {} • average {} • {} samples",
            format_value(observed.minimum, &sensor.unit),
            format_value(observed.maximum, &sensor.unit),
            format_value(observed.average, &sensor.unit),
            observed.samples
        );
    }
    let _ = writeln!(out, "  source: {}", sensor.source);
    let _ = writeln!(out, "  identification: {:?}", sensor.identification);
    if let Some(min) = sensor.min {
        let _ = writeln!(out, "  minimum: {}", format_value(min, &sensor.unit));
    }
    if let Some(max) = sensor.max {
        let _ = writeln!(out, "  maximum: {}", format_value(max, &sensor.unit));
    }
    if let Some(critical) = sensor.critical {
        let _ = writeln!(out, "  critical: {}", format_value(critical, &sensor.unit));
    }
    for (key, value) in &sensor.metadata {
        let _ = writeln!(
            out,
            "  {}: {}",
            humanize_key(key),
            property_to_string(value)
        );
    }
}
