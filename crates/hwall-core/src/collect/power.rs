use super::util::{add_string, basename, list_dirs, read_f64, read_trimmed};
use crate::model::{
    Device, DeviceClass, Identification, Sensor, SensorKind, SnapshotBuilder, Unit,
};
use std::path::Path;

pub(super) fn collect(builder: &mut SnapshotBuilder, include_sensitive: bool) {
    for path in list_dirs("/sys/class/power_supply") {
        let Some(name) = basename(&path) else {
            continue;
        };
        let supply_type =
            read_trimmed(path.join("type")).unwrap_or_else(|| "Power supply".to_owned());
        let class = if supply_type.eq_ignore_ascii_case("battery") {
            DeviceClass::Battery
        } else {
            DeviceClass::PowerSupply
        };
        let manufacturer = read_trimmed(path.join("manufacturer"));
        let model = read_trimmed(path.join("model_name"));
        let display_name = match (&manufacturer, &model) {
            (Some(vendor), Some(model)) => format!("{vendor} {model}"),
            (_, Some(model)) => model.clone(),
            _ => format!("{supply_type} {name}"),
        };

        let mut device = Device::new(format!("power:{name}"), class, display_name);
        device.vendor = manufacturer;
        device.model = model;
        device.bus_address = Some(name);
        add_string(&mut device.properties, "type", Some(supply_type));
        add_string(
            &mut device.properties,
            "status",
            read_trimmed(path.join("status")),
        );
        add_string(
            &mut device.properties,
            "technology",
            read_trimmed(path.join("technology")),
        );
        add_string(
            &mut device.properties,
            "health",
            read_trimmed(path.join("health")),
        );
        add_string(
            &mut device.properties,
            "scope",
            read_trimmed(path.join("scope")),
        );
        if include_sensitive {
            add_string(
                &mut device.properties,
                "serial",
                read_trimmed(path.join("serial_number")),
            );
        }

        for spec in POWER_SUPPLY_SENSORS {
            add_scaled_sensor(&mut device, &path, spec);
        }
        builder.add_device(device);
    }
}

struct PowerSupplySensor {
    file: &'static str,
    label: &'static str,
    kind: SensorKind,
    unit: Unit,
    divisor: f64,
}

const POWER_SUPPLY_SENSORS: &[PowerSupplySensor] = &[
    PowerSupplySensor {
        file: "capacity",
        label: "Capacity",
        kind: SensorKind::Capacity,
        unit: Unit::Percent,
        divisor: 1.0,
    },
    PowerSupplySensor {
        file: "voltage_now",
        label: "Voltage",
        kind: SensorKind::Voltage,
        unit: Unit::Volt,
        divisor: 1_000_000.0,
    },
    PowerSupplySensor {
        file: "voltage_min_design",
        label: "Design voltage",
        kind: SensorKind::Voltage,
        unit: Unit::Volt,
        divisor: 1_000_000.0,
    },
    PowerSupplySensor {
        file: "current_now",
        label: "Current",
        kind: SensorKind::Current,
        unit: Unit::Ampere,
        divisor: 1_000_000.0,
    },
    PowerSupplySensor {
        file: "power_now",
        label: "Power",
        kind: SensorKind::Power,
        unit: Unit::Watt,
        divisor: 1_000_000.0,
    },
    PowerSupplySensor {
        file: "energy_now",
        label: "Energy remaining",
        kind: SensorKind::Energy,
        unit: Unit::WattHour,
        divisor: 1_000_000.0,
    },
    PowerSupplySensor {
        file: "energy_full",
        label: "Energy full",
        kind: SensorKind::Energy,
        unit: Unit::WattHour,
        divisor: 1_000_000.0,
    },
    PowerSupplySensor {
        file: "energy_full_design",
        label: "Energy full design",
        kind: SensorKind::Energy,
        unit: Unit::WattHour,
        divisor: 1_000_000.0,
    },
    PowerSupplySensor {
        file: "charge_now",
        label: "Charge remaining",
        kind: SensorKind::Capacity,
        unit: Unit::AmpereHour,
        divisor: 1_000_000.0,
    },
    PowerSupplySensor {
        file: "temperature",
        label: "Temperature",
        kind: SensorKind::Temperature,
        unit: Unit::Celsius,
        divisor: 10.0,
    },
    PowerSupplySensor {
        file: "temp",
        label: "Temperature",
        kind: SensorKind::Temperature,
        unit: Unit::Celsius,
        divisor: 10.0,
    },
];

fn add_scaled_sensor(device: &mut Device, root: &Path, spec: &PowerSupplySensor) {
    let path = root.join(spec.file);
    let Some(raw) = read_f64(&path) else {
        return;
    };
    if device
        .sensors
        .iter()
        .any(|existing| existing.label == spec.label)
    {
        return;
    }

    device.sensors.push(Sensor::new(
        format!("{}:{}", device.id, spec.file),
        spec.label,
        spec.kind,
        spec.unit,
        Some(raw / spec.divisor),
        path.to_string_lossy(),
        Identification::KernelLabel,
    ));
}
