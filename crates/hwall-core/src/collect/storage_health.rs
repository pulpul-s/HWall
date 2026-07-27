use super::util::{
    basename, command_exists, is_nvme_controller_id, is_nvme_controller_name,
    is_virtual_block_device_name, list_dirs, read_bool01, run_command, run_command_elevated,
};
use crate::model::{
    Device, DeviceClass, Identification, PropertyValue, Sensor, SensorKind, SnapshotBuilder,
    StorageHealth, StorageHealthAvailability, StorageHealthStatus, Unit,
};
use serde_json::Value;
use std::process::Output;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub(crate) struct StorageHealthTarget {
    pub id: String,
    pub name: String,
    pub model: Option<String>,
    pub driver: Option<String>,
    pub device_path: String,
    pub nvme_path: Option<String>,
    pub rotational: bool,
}

#[derive(Debug)]
enum HelperFailure {
    PermissionDenied(String),
    DriveAsleep(String),
    HelperMissing(String),
    Unsupported(String),
    Error(String),
}

impl HelperFailure {
    fn into_health(self, attempted_at: u128) -> StorageHealth {
        let (availability, message) = match self {
            Self::PermissionDenied(message) => {
                (StorageHealthAvailability::PermissionDenied, message)
            }
            Self::DriveAsleep(message) => (StorageHealthAvailability::DriveAsleep, message),
            Self::HelperMissing(message) => (StorageHealthAvailability::HelperMissing, message),
            Self::Unsupported(message) => (StorageHealthAvailability::Unsupported, message),
            Self::Error(message) => (StorageHealthAvailability::Error, message),
        };
        StorageHealth {
            status: StorageHealthStatus::Unknown,
            availability,
            last_attempt_unix_ms: Some(attempted_at),
            last_success_unix_ms: None,
            message: Some(message),
            sources: Vec::new(),
        }
    }
}

pub(super) fn collect(
    builder: &mut SnapshotBuilder,
    allow_helper_commands: bool,
    include_sensitive: bool,
) {
    if !allow_helper_commands {
        builder.warn("Storage health requested, but helper commands were disabled.");
        return;
    }
    let targets = discover_targets();
    collect_targets(builder, &targets, include_sensitive, false);
}

pub(crate) fn target_from_device(device: &Device) -> Option<StorageHealthTarget> {
    if device.class != DeviceClass::Storage || device.property_bool("partition").unwrap_or(false) {
        return None;
    }
    let id_name = device.id.strip_prefix("block:")?;
    if is_virtual_block_device_name(id_name) {
        return None;
    }
    if id_name.starts_with("nvme") && device.parent.as_deref().is_some_and(is_nvme_controller_id) {
        return None;
    }
    let controller_name = device
        .property_str("controller_name")
        .filter(|name| is_nvme_controller_name(name))
        .or_else(|| is_nvme_controller_name(id_name).then_some(id_name));
    let path_name = controller_name
        .or(device.bus_address.as_deref())
        .unwrap_or(id_name);
    Some(StorageHealthTarget {
        id: device.id.clone(),
        name: device.name.clone(),
        model: device.model.clone(),
        driver: device.driver.clone(),
        device_path: format!("/dev/{path_name}"),
        nvme_path: controller_name.map(|name| format!("/dev/{name}")),
        rotational: if controller_name.is_some() {
            false
        } else {
            device.property_bool("rotational").unwrap_or(true)
        },
    })
}

pub(crate) fn collect_targets(
    builder: &mut SnapshotBuilder,
    targets: &[StorageHealthTarget],
    include_sensitive: bool,
    elevated: bool,
) {
    for target in targets {
        collect_target(builder, target, include_sensitive, elevated);
    }
}

fn collect_target(
    builder: &mut SnapshotBuilder,
    target: &StorageHealthTarget,
    include_sensitive: bool,
    elevated: bool,
) {
    let attempted_at = unix_time_ms();
    let mut device = Device::new(&target.id, DeviceClass::Storage, &target.name);
    device.model = target.model.clone();
    device.driver = target.driver.clone();

    let result = if target.nvme_path.is_some() {
        collect_nvme(&mut device, target, elevated, attempted_at).map(|status| ("nvme-cli", status))
    } else {
        collect_smartctl(
            &mut device,
            target,
            include_sensitive,
            elevated,
            attempted_at,
        )
        .map(|status| ("smartctl", status))
    };
    device.storage_health = Some(match result {
        Ok((source, status)) => StorageHealth {
            status,
            availability: StorageHealthAvailability::Current,
            last_attempt_unix_ms: Some(attempted_at),
            last_success_unix_ms: Some(attempted_at),
            message: None,
            sources: vec![source.to_owned()],
        },
        Err(error) => error.into_health(attempted_at),
    });
    builder.add_device(device);
}

fn collect_smartctl(
    device: &mut Device,
    target: &StorageHealthTarget,
    include_sensitive: bool,
    elevated: bool,
    sampled_at: u128,
) -> Result<StorageHealthStatus, HelperFailure> {
    if !command_exists("smartctl") {
        return Err(HelperFailure::HelperMissing(
            "smartctl was not found.".to_owned(),
        ));
    }
    if elevated && !command_exists("pkexec") {
        return Err(HelperFailure::HelperMissing(
            "pkexec was not found, so an administrator retry is unavailable.".to_owned(),
        ));
    }

    let mut args = Vec::new();
    if target.rotational {
        args.extend(["--nocheck=standby", "--all", "--json"]);
    } else {
        args.extend(["--all", "--json"]);
    }
    args.push(target.device_path.as_str());
    let output = run_helper("smartctl", args, elevated).map_err(|error| {
        HelperFailure::Error(format!("smartctl could not be started: {error}."))
    })?;
    let text = output_text(&output);
    let normalized = text.to_ascii_lowercase();
    if is_permission_denied(&normalized) {
        return Err(HelperFailure::PermissionDenied(format!(
            "smartctl could not open {} because permission was denied.",
            target.device_path
        )));
    }
    if target.rotational && is_drive_asleep(&normalized) {
        return Err(HelperFailure::DriveAsleep(format!(
            "{} is asleep; HWall did not wake it.",
            target.device_path
        )));
    }
    if is_unsupported(&normalized) {
        return Err(HelperFailure::Unsupported(format!(
            "SMART health is not supported for {}.",
            target.device_path
        )));
    }

    let json = serde_json::from_slice::<Value>(&output.stdout).map_err(|error| {
        HelperFailure::Error(format!("smartctl returned invalid JSON: {error}."))
    })?;
    let mut found = false;
    if let Some(model) = json.get("model_name").and_then(Value::as_str) {
        device.model = Some(model.to_owned());
        found = true;
    }
    if let Some(firmware) = json.get("firmware_version").and_then(Value::as_str) {
        device
            .properties
            .insert("firmware_revision".to_owned(), firmware.into());
        found = true;
    }
    if include_sensitive {
        if let Some(serial) = json.get("serial_number").and_then(Value::as_str) {
            device.properties.insert("serial".to_owned(), serial.into());
            found = true;
        }
        if let Some(wwn) = json.pointer("/wwn/naa").and_then(number_as_u64) {
            device.properties.insert("wwid".to_owned(), wwn.into());
            found = true;
        }
    }

    let status = json
        .pointer("/smart_status/passed")
        .and_then(Value::as_bool)
        .map(|passed| {
            found = true;
            if passed {
                StorageHealthStatus::Passed
            } else {
                StorageHealthStatus::Failed
            }
        });
    if let Some(hours) = json.pointer("/power_on_time/hours").and_then(number_as_u64) {
        device
            .properties
            .insert("power_on_hours".to_owned(), hours.into());
        found = true;
    }
    if let Some(cycles) = json.get("power_cycle_count").and_then(number_as_u64) {
        device
            .properties
            .insert("power_cycles".to_owned(), cycles.into());
        found = true;
    }
    if let Some(temperature) = json.pointer("/temperature/current").and_then(number_as_f64) {
        add_intermittent_temperature(
            device,
            format!("{}:smart:temperature", target.id),
            "Drive temperature",
            temperature,
            "smartctl --json",
            sampled_at,
        );
        found = true;
    }

    if !found {
        return Err(HelperFailure::Error(helper_error(
            "smartctl",
            &target.device_path,
            &text,
        )));
    }
    Ok(status.unwrap_or(StorageHealthStatus::Unknown))
}

fn collect_nvme(
    device: &mut Device,
    target: &StorageHealthTarget,
    elevated: bool,
    sampled_at: u128,
) -> Result<StorageHealthStatus, HelperFailure> {
    if !command_exists("nvme") {
        return Err(HelperFailure::HelperMissing(
            "nvme-cli was not found.".to_owned(),
        ));
    }
    if elevated && !command_exists("pkexec") {
        return Err(HelperFailure::HelperMissing(
            "pkexec was not found, so an administrator retry is unavailable.".to_owned(),
        ));
    }
    let path = target.nvme_path.as_deref().unwrap_or(&target.device_path);
    let output = run_helper("nvme", ["smart-log", "-o", "json", path], elevated)
        .map_err(|error| HelperFailure::Error(format!("nvme could not be started: {error}.")))?;
    let text = output_text(&output);
    let normalized = text.to_ascii_lowercase();
    if is_permission_denied(&normalized) {
        return Err(HelperFailure::PermissionDenied(format!(
            "nvme-cli could not open {path} because permission was denied."
        )));
    }
    if is_unsupported(&normalized) {
        return Err(HelperFailure::Unsupported(format!(
            "NVMe health is not supported for {path}."
        )));
    }
    if !output.status.success() {
        return Err(HelperFailure::Error(helper_error("nvme", path, &text)));
    }
    let json = serde_json::from_slice::<Value>(&output.stdout)
        .map_err(|error| HelperFailure::Error(format!("nvme returned invalid JSON: {error}.")))?;
    let property_count = device.properties.len();
    let sensor_count = device.sensors.len();
    let status = apply_nvme_json(device, target, &json, sampled_at);
    if device.properties.len() == property_count && device.sensors.len() == sensor_count {
        return Err(HelperFailure::Error(
            "nvme returned JSON without recognized health fields.".to_owned(),
        ));
    }

    Ok(status)
}

fn apply_nvme_json(
    device: &mut Device,
    target: &StorageHealthTarget,
    json: &Value,
    sampled_at: u128,
) -> StorageHealthStatus {
    let critical_warning = json.get("critical_warning").and_then(number_as_u64);
    if let Some(critical_warning) = critical_warning {
        device
            .properties
            .insert("critical_warning".to_owned(), critical_warning.into());
        let warning_details = critical_warning_details(critical_warning);
        if !warning_details.is_empty() {
            device.properties.insert(
                "critical_warning_details".to_owned(),
                PropertyValue::Strings(warning_details),
            );
        }
    }

    for (source, target_key) in [
        ("avail_spare", "available_spare"),
        ("spare_thresh", "spare_threshold"),
        ("percent_used", "percentage_used"),
        ("percentage_used", "percentage_used"),
        ("data_units_read", "data_units_read"),
        ("data_units_written", "data_units_written"),
        ("host_read_commands", "host_read_commands"),
        ("host_write_commands", "host_write_commands"),
        ("power_cycles", "power_cycles"),
        ("power_on_hours", "power_on_hours"),
        ("unsafe_shutdowns", "unsafe_shutdowns"),
        ("media_errors", "media_errors"),
        ("num_err_log_entries", "error_log_entries"),
        ("warning_temp_time", "warning_temperature_time"),
        ("critical_comp_time", "critical_temperature_time"),
    ] {
        if device.properties.contains_key(target_key) && source == "percentage_used" {
            continue;
        }
        if let Some(value) = json.get(source).and_then(number_as_u64) {
            device
                .properties
                .insert(target_key.to_owned(), PropertyValue::Unsigned(value));
        }
    }

    if let Some(raw_temperature) = json.get("temperature").and_then(number_as_f64) {
        let celsius = if raw_temperature > 200.0 {
            raw_temperature - 273.15
        } else {
            raw_temperature
        };
        add_intermittent_temperature(
            device,
            format!("{}:nvme:temperature", target.id),
            "Controller temperature",
            celsius,
            "nvme smart-log",
            sampled_at,
        );
    }

    match critical_warning {
        Some(0) => StorageHealthStatus::Passed,
        Some(_) => StorageHealthStatus::Warning,
        None => StorageHealthStatus::Unknown,
    }
}

fn discover_targets() -> Vec<StorageHealthTarget> {
    let mut targets = Vec::new();
    for controller in list_dirs("/sys/class/nvme") {
        let Some(name) = basename(&controller) else {
            continue;
        };
        targets.push(StorageHealthTarget {
            id: format!("block:{name}"),
            name: format!("NVMe controller {name}"),
            model: None,
            driver: Some("nvme".to_owned()),
            device_path: format!("/dev/{name}"),
            nvme_path: Some(format!("/dev/{name}")),
            rotational: false,
        });
    }
    for block in list_dirs("/sys/class/block") {
        let Some(name) = basename(&block) else {
            continue;
        };
        if name.starts_with("nvme")
            || block.join("partition").exists()
            || !block.join("device").exists()
            || is_virtual_block_device_name(&name)
        {
            continue;
        }
        targets.push(StorageHealthTarget {
            id: format!("block:{name}"),
            name: name.clone(),
            model: None,
            driver: None,
            device_path: format!("/dev/{name}"),
            nvme_path: None,
            rotational: read_bool01(block.join("queue/rotational")).unwrap_or(true),
        });
    }
    targets
}

fn run_helper<I, S>(program: &str, args: I, elevated: bool) -> std::io::Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    if elevated {
        run_command_elevated(program, args)
    } else {
        run_command(program, args)
    }
}

fn add_intermittent_temperature(
    device: &mut Device,
    id: String,
    label: &str,
    value: f64,
    source: &str,
    sampled_at: u128,
) {
    let mut sensor = Sensor::new(
        id,
        label,
        SensorKind::Temperature,
        Unit::Celsius,
        Some(value),
        source,
        Identification::VendorApi,
    );
    sensor
        .metadata
        .insert("intermittent".to_owned(), true.into());
    sensor.metadata.insert(
        "sampled_at_unix_ms".to_owned(),
        PropertyValue::Unsigned(u64::try_from(sampled_at).unwrap_or(u64::MAX)),
    );
    device.sensors.push(sensor);
}

fn critical_warning_details(bits: u64) -> Vec<String> {
    let mut details = Vec::new();
    for (bit, label) in [
        (0, "Available spare below threshold"),
        (1, "Temperature threshold exceeded"),
        (2, "Reliability degraded"),
        (3, "Media placed in read-only mode"),
        (4, "Volatile memory backup failed"),
        (5, "Persistent memory region is read-only"),
    ] {
        if bits & (1 << bit) != 0 {
            details.push(label.to_owned());
        }
    }
    details
}

fn output_text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn is_permission_denied(text: &str) -> bool {
    [
        "permission denied",
        "operation not permitted",
        "not authorized",
        "authentication failed",
        "authorization failed",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn is_drive_asleep(text: &str) -> bool {
    (text.contains("standby") || text.contains("sleep"))
        && (text.contains("exit") || text.contains("skip") || text.contains("low-power"))
}

fn is_unsupported(text: &str) -> bool {
    text.contains("unsupported")
        || text.contains("not supported")
        || text.contains("smart support is: unavailable")
}

fn helper_error(helper: &str, path: &str, text: &str) -> String {
    let detail = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("unknown error")
        .trim_end_matches('.');
    format!("{helper} could not read {path}: {detail}.")
}

fn number_as_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.trim().parse::<f64>().ok())
}

fn number_as_u64(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        let text = value.as_str()?.trim();
        let compact = text.replace(',', "");
        compact
            .strip_prefix("0x")
            .or_else(|| compact.strip_prefix("0X"))
            .and_then(|hex| u64::from_str_radix(hex, 16).ok())
            .or_else(|| compact.parse::<u64>().ok())
    })
}

fn unix_time_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nvme_target() -> StorageHealthTarget {
        StorageHealthTarget {
            id: "block:nvme2".to_owned(),
            name: "Example NVMe".to_owned(),
            model: None,
            driver: Some("nvme".to_owned()),
            device_path: "/dev/nvme2".to_owned(),
            nvme_path: Some("/dev/nvme2".to_owned()),
            rotational: false,
        }
    }

    #[test]
    fn parses_current_nvme_json_into_canonical_properties() {
        let json: Value = serde_json::from_str(
            r#"{
                "critical_warning": 0,
                "avail_spare": 100,
                "spare_thresh": 10,
                "percent_used": 7,
                "power_cycles": 12,
                "temperature": 316
            }"#,
        )
        .unwrap();
        let mut device = Device::new("block:nvme2", DeviceClass::Storage, "Example NVMe");

        let status = apply_nvme_json(&mut device, &nvme_target(), &json, 1_000);

        assert_eq!(status, StorageHealthStatus::Passed);
        assert_eq!(
            device.properties.get("percentage_used"),
            Some(&PropertyValue::Unsigned(7))
        );
        assert_eq!(
            device.properties.get("power_cycles"),
            Some(&PropertyValue::Unsigned(12))
        );
        assert_eq!(device.sensors.len(), 1);
        assert!(matches!(
            device.sensors[0].metadata.get("intermittent"),
            Some(PropertyValue::Boolean(true))
        ));
    }

    #[test]
    fn accepts_legacy_percentage_key_and_string_numbers() {
        let json: Value =
            serde_json::from_str(r#"{"critical_warning":"0x0","percentage_used":"8"}"#).unwrap();
        let mut device = Device::new("block:nvme2", DeviceClass::Storage, "Example NVMe");

        apply_nvme_json(&mut device, &nvme_target(), &json, 1_000);

        assert_eq!(
            device.properties.get("percentage_used"),
            Some(&PropertyValue::Unsigned(8))
        );
    }

    #[test]
    fn missing_critical_warning_does_not_imply_passed_health() {
        let json: Value = serde_json::from_str(r#"{"percent_used":7}"#).unwrap();
        let mut device = Device::new("block:nvme2", DeviceClass::Storage, "Example NVMe");

        let status = apply_nvme_json(&mut device, &nvme_target(), &json, 1_000);

        assert_eq!(status, StorageHealthStatus::Unknown);
        assert!(!device.properties.contains_key("critical_warning"));
    }

    #[test]
    fn decodes_nvme_critical_warning_bits() {
        assert!(critical_warning_details(0).is_empty());
        assert_eq!(
            critical_warning_details((1 << 0) | (1 << 3)),
            vec![
                "Available spare below threshold".to_owned(),
                "Media placed in read-only mode".to_owned(),
            ]
        );
    }

    #[test]
    fn recognizes_controller_names_only() {
        assert!(is_nvme_controller_name("nvme2"));
        assert!(!is_nvme_controller_name("nvme2n1"));
    }
}
