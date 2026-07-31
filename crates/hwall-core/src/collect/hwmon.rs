use super::ownership::{
    configure_hwmon_device, default_sensor_label, hwmon_display_name, resolve_device, DriverProfile,
};
use super::util::{
    canonical, command_exists, humanize_token, list_dirs, list_entries, pci_address_from_path,
    read_f64, read_trimmed, read_u64, run_command,
};
use crate::model::{
    CollectorId, Device, DeviceClass, Identification, Sensor, SensorKind, SensorStatus,
    SnapshotBuilder, Unit,
};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

pub(super) fn collect(builder: &mut SnapshotBuilder, use_libsensors: bool) {
    let libsensors = if use_libsensors {
        match load_libsensors_chips() {
            Ok(chips) => {
                builder.mark_collector_succeeded(CollectorId::LibSensors);
                chips
            }
            Err(()) => {
                builder.mark_collector_failed(CollectorId::LibSensors);
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    for root in list_dirs("/sys/class/hwmon") {
        collect_chip(builder, &root, &libsensors);
    }
}

#[derive(Debug)]
struct LibSensorsChip {
    prefix: String,
    pci_token: Option<String>,
    values: HashMap<String, LibSensorsValue>,
}

#[derive(Debug)]
struct LibSensorsValue {
    label: String,
    value: f64,
}

fn collect_chip(builder: &mut SnapshotBuilder, root: &Path, libsensors: &[LibSensorsChip]) {
    let hwmon_name = read_trimmed(root.join("name")).unwrap_or_else(|| {
        root.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("hwmon")
            .to_owned()
    });
    let device_path = canonical(root.join("device")).unwrap_or_else(|| root.to_path_buf());
    let resolved = resolve_device(&device_path, Some(&hwmon_name));
    let device_id = resolved.id;
    let class = match resolved.class {
        DeviceClass::Other => DeviceClass::SensorController,
        other => other,
    };
    let mut device = Device::new(
        device_id.clone(),
        class,
        hwmon_display_name(&hwmon_name, resolved.profile, &device_path),
    );
    // The hwmon driver describes the telemetry provider, not necessarily the
    // physical device's primary driver. Keep it as explicit metadata so CPU
    // frequency drivers, GPU drivers, and storage drivers are not overwritten
    // when records merge.
    device
        .properties
        .insert("hwmon_driver".to_owned(), hwmon_name.clone().into());
    if matches!(class, DeviceClass::Memory | DeviceClass::SensorController) {
        device.driver = Some(hwmon_name.clone());
    }
    configure_hwmon_device(&mut device, resolved.profile, &device_path);
    device.properties.insert(
        "hwmon_path".to_owned(),
        root.to_string_lossy().to_string().into(),
    );
    device.properties.insert(
        "device_path".to_owned(),
        device_path.to_string_lossy().to_string().into(),
    );
    if let Some(interval) = read_f64(root.join("update_interval")) {
        device
            .properties
            .insert("update_interval_ms".to_owned(), interval.into());
    }
    if let Some(interval) = read_f64(root.join("update_interval_us")) {
        device
            .properties
            .insert("update_interval_us".to_owned(), interval.into());
    }
    if matches!(&device.class, DeviceClass::SensorController) {
        device.parent = Some("motherboard:0".to_owned());
    }

    let selected_chip = select_libsensors_chip(libsensors, &hwmon_name, &device_path);
    for path in list_entries(root) {
        let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(spec) = SensorSpec::from_filename(filename) else {
            continue;
        };
        let Some(raw) = read_f64(&path) else {
            continue;
        };
        let override_value = selected_chip.and_then(|chip| chip.values.get(filename));
        let (label, identification) = resolve_label(root, &spec, override_value, resolved.profile);
        let value = override_value
            .map(|value| value.value)
            .unwrap_or(raw / spec.divisor);
        let sensor_id = format!("{device_id}:hwmon:{hwmon_name}:{}", spec.metric_id);
        let mut sensor = Sensor::new(
            sensor_id.clone(),
            label,
            spec.kind,
            spec.unit,
            Some(value),
            path.to_string_lossy(),
            identification,
        );
        sensor.raw_value = Some(raw.to_string());
        sensor
            .metadata
            .insert("driver".to_owned(), hwmon_name.clone().into());
        let raw_min = read_f64(root.join(format!("{}_min", spec.base)));
        let raw_max = read_f64(root.join(format!("{}_max", spec.base)));
        let raw_critical = read_f64(root.join(format!("{}_crit", spec.base)))
            .or_else(|| read_f64(root.join(format!("{}_emergency", spec.base))));
        if override_value.is_none() {
            sensor.min = raw_min.map(|value| value / spec.divisor);
            sensor.max = raw_max.map(|value| value / spec.divisor);
            sensor.critical = raw_critical.map(|value| value / spec.divisor);
        } else {
            sensor
                .metadata
                .insert("computed_by".to_owned(), "lm-sensors".into());
        }
        apply_status(
            &mut sensor,
            root,
            &spec.base,
            raw_min,
            raw_max,
            raw_critical,
        );
        sensor
            .metadata
            .insert("hwmon_name".to_owned(), hwmon_name.clone().into());
        sensor
            .metadata
            .insert("attribute".to_owned(), filename.to_owned().into());
        if let Some(channel) = spec.channel {
            sensor
                .metadata
                .insert("hwmon_channel".to_owned(), u64::from(channel).into());
        }
        if spec.kind == SensorKind::Energy && !spec.is_average {
            if let Some(raw_counter) = read_u64(&path) {
                let counter_key = format!("hwmon_energy_uj:{sensor_id}");
                device.counters.insert(counter_key.clone(), raw_counter);
                sensor
                    .metadata
                    .insert("energy_counter_key".to_owned(), counter_key.into());
            }
        }
        if spec.is_average {
            sensor
                .metadata
                .insert("aggregation".to_owned(), "average".into());
        }
        sensor.mark_collector(CollectorId::Hwmon);
        device.sensors.push(sensor);
    }

    if !device.sensors.is_empty() || !device.properties.is_empty() {
        builder.add_device(device);
    }
}

#[derive(Debug)]
struct SensorSpec {
    base: String,
    metric_id: String,
    kind: SensorKind,
    unit: Unit,
    divisor: f64,
    is_average: bool,
    channel: Option<u32>,
}

impl SensorSpec {
    fn from_filename(filename: &str) -> Option<Self> {
        let (base, is_average) = if let Some(base) = filename.strip_suffix("_input") {
            (base, false)
        } else if let Some(base) = filename.strip_suffix("_average") {
            (base, true)
        } else if is_pwm_attribute(filename) {
            (filename, false)
        } else {
            return None;
        };
        let prefix = base
            .chars()
            .take_while(|ch| ch.is_ascii_alphabetic())
            .collect::<String>();
        let (kind, unit, divisor) = match prefix.as_str() {
            "temp" => (SensorKind::Temperature, Unit::Celsius, 1_000.0),
            "in" => (SensorKind::Voltage, Unit::Volt, 1_000.0),
            "curr" => (SensorKind::Current, Unit::Ampere, 1_000.0),
            "power" => (SensorKind::Power, Unit::Watt, 1_000_000.0),
            "energy" => (SensorKind::Energy, Unit::Joule, 1_000_000.0),
            "fan" => (SensorKind::Fan, Unit::Rpm, 1.0),
            "pwm" => (SensorKind::Fan, Unit::Percent, 255.0 / 100.0),
            "freq" => (SensorKind::Frequency, Unit::Hertz, 1.0),
            "humidity" => (SensorKind::Humidity, Unit::Percent, 1_000.0),
            "intrusion" => (SensorKind::Boolean, Unit::Boolean, 1.0),
            _ => return None,
        };
        Some(Self {
            base: base.to_owned(),
            metric_id: filename.to_owned(),
            kind,
            unit,
            divisor,
            is_average,
            channel: channel_number(base),
        })
    }
}

fn is_pwm_attribute(filename: &str) -> bool {
    filename
        .strip_prefix("pwm")
        .is_some_and(|number| !number.is_empty() && number.chars().all(|ch| ch.is_ascii_digit()))
}

fn channel_number(base: &str) -> Option<u32> {
    let number = base
        .chars()
        .skip_while(|character| character.is_ascii_alphabetic())
        .collect::<String>();
    number.parse().ok()
}

fn resolve_label(
    root: &Path,
    spec: &SensorSpec,
    override_value: Option<&LibSensorsValue>,
    profile: Option<&DriverProfile>,
) -> (String, Identification) {
    if let Some(label) = read_trimmed(root.join(format!("{}_label", spec.base))) {
        let label = if spec.is_average {
            format!("{label} average")
        } else {
            label
        };
        return (label, Identification::KernelLabel);
    }
    if let Some(value) = override_value {
        let label = if spec.is_average {
            format!("{} average", value.label)
        } else {
            value.label.clone()
        };
        return (label, Identification::LibSensorsConfig);
    }
    if let Some(label) = default_sensor_label(profile, &spec.kind, spec.channel, spec.is_average) {
        return label;
    }
    let number = spec
        .base
        .chars()
        .skip_while(|ch| ch.is_ascii_alphabetic())
        .collect::<String>();
    let prefix = spec
        .base
        .chars()
        .take_while(|ch| ch.is_ascii_alphabetic())
        .collect::<String>();
    let generic = match prefix.as_str() {
        "temp" => format!("Temperature {number}"),
        "in" => format!("Voltage {number}"),
        "curr" => format!("Current {number}"),
        "power" => format!("Power {number}"),
        "energy" => format!("Energy {number}"),
        "fan" => format!("Fan {number}"),
        "pwm" => format!("PWM {number}"),
        "freq" => format!("Frequency {number}"),
        "humidity" => format!("Humidity {number}"),
        "intrusion" => format!("Intrusion {number}"),
        _ => humanize_token(&spec.base),
    };
    let generic = if spec.is_average {
        format!("{generic} average")
    } else {
        generic
    };
    (generic, Identification::Unidentified)
}

fn apply_status(
    sensor: &mut Sensor,
    root: &Path,
    base: &str,
    raw_min: Option<f64>,
    raw_max: Option<f64>,
    raw_critical: Option<f64>,
) {
    let raw_fault = read_trimmed(root.join(format!("{base}_fault"))).as_deref() == Some("1");
    let raw_alarm = read_trimmed(root.join(format!("{base}_alarm"))).as_deref() == Some("1");
    let (status, unconfigured_alarm) = classify_status(
        sensor.kind,
        raw_fault,
        raw_alarm,
        [raw_min, raw_max, raw_critical],
    );
    sensor.status = status;

    if raw_fault {
        sensor.metadata.insert("raw_fault".to_owned(), true.into());
    }
    if raw_alarm {
        sensor.metadata.insert("raw_alarm".to_owned(), true.into());
    }
    if unconfigured_alarm {
        sensor.metadata.insert(
            "alarm_note".to_owned(),
            "hardware alarm set, but limits are not configured".into(),
        );
    }
}

fn classify_status(
    kind: SensorKind,
    raw_fault: bool,
    raw_alarm: bool,
    thresholds: [Option<f64>; 3],
) -> (SensorStatus, bool) {
    if raw_fault {
        return (SensorStatus::Fault, false);
    }
    if !raw_alarm {
        return (SensorStatus::Ok, false);
    }

    // Boolean alarm channels such as chassis intrusion are meaningful without
    // numeric limits. For numeric channels, all-zero/missing thresholds are a
    // common firmware default and must not turn every normal reading into an
    // orange warning.
    if kind == SensorKind::Boolean {
        return (SensorStatus::Alarm, false);
    }

    let limits_configured = thresholds
        .into_iter()
        .flatten()
        .any(|value| value.is_finite() && value.abs() > f64::EPSILON);
    if limits_configured {
        (SensorStatus::Alarm, false)
    } else {
        (SensorStatus::Ok, true)
    }
}

fn select_libsensors_chip<'a>(
    chips: &'a [LibSensorsChip],
    hwmon_name: &str,
    device_path: &Path,
) -> Option<&'a LibSensorsChip> {
    let candidates = chips
        .iter()
        .filter(|chip| chip.prefix == hwmon_name)
        .collect::<Vec<_>>();
    if candidates.len() == 1 {
        return candidates.into_iter().next();
    }
    let pci_token = pci_address_from_path(device_path).and_then(|address| pci_token(&address));
    pci_token.and_then(|token| {
        candidates
            .into_iter()
            .find(|chip| chip.pci_token.as_deref() == Some(token.as_str()))
    })
}

fn pci_token(address: &str) -> Option<String> {
    let mut parts = address.split([':', '.']);
    let _domain = parts.next()?;
    let bus = parts.next()?;
    let device = parts.next()?;
    Some(format!("{bus}{device}"))
}

fn load_libsensors_chips() -> Result<Vec<LibSensorsChip>, ()> {
    if !command_exists("sensors") {
        return Err(());
    }
    let output = run_command("sensors", ["-j"]).map_err(|_| ())?;
    if !output.status.success() {
        return Err(());
    }
    let value = serde_json::from_slice::<Value>(&output.stdout).map_err(|_| ())?;
    let chips = value.as_object().ok_or(())?;
    let mut parsed = Vec::new();
    for (chip_name, chip_value) in chips {
        let prefix = chip_name.split('-').next().unwrap_or(chip_name).to_owned();
        let pci_token = chip_name
            .split_once("-pci-")
            .map(|(_, token)| token.to_ascii_lowercase());
        let Some(features) = chip_value.as_object() else {
            continue;
        };
        let mut values = HashMap::new();
        for (feature_label, feature_value) in features {
            if feature_label == "Adapter" {
                continue;
            }
            let Some(attributes) = feature_value.as_object() else {
                continue;
            };
            for (attribute, numeric_value) in attributes {
                if !(attribute.ends_with("_input") || attribute.ends_with("_average")) {
                    continue;
                }
                if let Some(value) = numeric_value.as_f64() {
                    values.insert(
                        attribute.clone(),
                        LibSensorsValue {
                            label: feature_label.clone(),
                            value,
                        },
                    );
                }
            }
        }
        if !values.is_empty() {
            parsed.push(LibSensorsChip {
                prefix,
                pci_token,
                values,
            });
        }
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_lm_sensors_pci_token() {
        assert_eq!(pci_token("0000:03:00.0").as_deref(), Some("0300"));
    }

    #[test]
    fn recognizes_standard_inputs() {
        let temp = SensorSpec::from_filename("temp2_input").unwrap();
        assert_eq!(temp.kind, SensorKind::Temperature);
        assert_eq!(temp.divisor, 1_000.0);
        assert!(SensorSpec::from_filename("temp2_label").is_none());
    }

    #[test]
    fn recognizes_pwm_duty_cycles() {
        let pwm = SensorSpec::from_filename("pwm3").unwrap();
        assert_eq!(pwm.kind, SensorKind::Fan);
        assert_eq!(pwm.unit, Unit::Percent);
        assert_eq!(pwm.channel, Some(3));
        assert_eq!(pwm.metric_id, "pwm3");
        assert!((51.0 / pwm.divisor - 20.0).abs() < 0.000_001);
        assert!((255.0 / pwm.divisor - 100.0).abs() < 0.000_001);
        assert!(SensorSpec::from_filename("pwm3_enable").is_none());
        assert!(SensorSpec::from_filename("pwm3_mode").is_none());

        let (label, identification) =
            resolve_label(Path::new("/definitely/not/a/real/hwmon"), &pwm, None, None);

        assert_eq!(label, "PWM 3");
        assert_eq!(identification, Identification::Unidentified);
    }

    #[test]
    fn gives_spd5118_temperature_a_semantic_label() {
        let temp = SensorSpec::from_filename("temp1_input").unwrap();
        let (label, identification) = resolve_label(
            Path::new("/definitely/not/a/real/hwmon"),
            &temp,
            None,
            super::super::ownership::resolve_device(
                Path::new("/sys/devices/i2c-6/6-0053"),
                Some("spd5118"),
            )
            .profile,
        );
        assert_eq!(label, "Module temperature");
        assert_eq!(identification, Identification::KnownDriverMapping);
    }

    #[test]
    fn ignores_numeric_alarm_when_limits_are_unconfigured() {
        assert_eq!(
            classify_status(
                SensorKind::Voltage,
                false,
                true,
                [Some(0.0), Some(0.0), None],
            ),
            (SensorStatus::Ok, true),
        );
    }

    #[test]
    fn keeps_boolean_alarm_without_numeric_limits() {
        assert_eq!(
            classify_status(SensorKind::Boolean, false, true, [None, None, None]),
            (SensorStatus::Alarm, false),
        );
    }

    #[test]
    fn keeps_alarm_when_a_real_limit_exists() {
        assert_eq!(
            classify_status(
                SensorKind::Temperature,
                false,
                true,
                [None, Some(80_000.0), Some(100_000.0)],
            ),
            (SensorStatus::Alarm, false),
        );
    }
}
