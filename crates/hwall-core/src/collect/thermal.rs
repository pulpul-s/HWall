use super::util::{basename, list_dirs, read_f64, read_trimmed};
use crate::model::{
    Device, DeviceClass, Identification, Sensor, SensorKind, SnapshotBuilder, Unit,
};

pub(super) fn collect(builder: &mut SnapshotBuilder) {
    for path in list_dirs("/sys/class/thermal") {
        let Some(name) = basename(&path) else {
            continue;
        };
        if !name.starts_with("thermal_zone") {
            continue;
        }
        let zone_type = read_trimmed(path.join("type")).unwrap_or_else(|| name.clone());
        let mut device = Device::new(
            format!("thermal:{name}"),
            DeviceClass::Thermal,
            format!("Thermal zone: {zone_type}"),
        );
        device.bus_address = Some(name.clone());
        if let Some(raw) = read_f64(path.join("temp")) {
            let sensor = Sensor::new(
                format!("thermal:{name}:temperature"),
                zone_type,
                SensorKind::Temperature,
                Unit::Celsius,
                Some(raw / 1_000.0),
                path.join("temp").to_string_lossy(),
                Identification::FirmwareLabel,
            );
            device.sensors.push(sensor);
        }
        builder.add_device(device);
    }
}
