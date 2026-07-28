use crate::{
    is_storage_health_property, Identification, PropertyValue, Sensor, SensorKind, SensorStatus,
    Unit,
};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn sensor_status_suffix(sensor: &Sensor) -> String {
    let mut suffix = match sensor.status {
        SensorStatus::Ok => String::new(),
        SensorStatus::Alarm => "  ALARM".to_owned(),
        SensorStatus::Fault => "  FAULT".to_owned(),
        SensorStatus::Unavailable => "  unavailable".to_owned(),
    };
    match sensor.identification {
        Identification::Unidentified => suffix.push_str("  [unidentified]"),
        Identification::Inferred => suffix.push_str("  [inferred]"),
        _ => {}
    }
    if let Some(sampled_at) = sensor.sampled_at_unix_ms() {
        suffix.push_str(&format!(
            "  [sampled {}]",
            format_sample_age_compact(sampled_at)
        ));
    }
    suffix
}

fn sample_age_seconds(timestamp_ms: u128) -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .saturating_sub(timestamp_ms)
        / 1_000
}

pub fn format_sample_age_compact(timestamp_ms: u128) -> String {
    match sample_age_seconds(timestamp_ms) {
        0..=59 => "just now".to_owned(),
        seconds @ 60..=3_599 => format!("{} min ago", seconds / 60),
        seconds @ 3_600..=86_399 => format!("{} h ago", seconds / 3_600),
        seconds => format!("{} d ago", seconds / 86_400),
    }
}

pub fn format_sample_age(timestamp_ms: u128) -> String {
    match sample_age_seconds(timestamp_ms) {
        0..=59 => "Just now".to_owned(),
        seconds @ 60..=3_599 => format!("{} minutes ago", seconds / 60),
        seconds @ 3_600..=86_399 => format!("{} hours ago", seconds / 3_600),
        seconds => format!("{} days ago", seconds / 86_400),
    }
}

pub fn storage_health_property_label(key: &str) -> &str {
    match key {
        "critical_warning" => "Critical warning",
        "critical_warning_details" => "Critical warning details",
        "available_spare" => "Available spare",
        "spare_threshold" => "Spare threshold",
        "percentage_used" => "Lifetime used",
        "power_on_hours" => "Power-on hours",
        "power_cycles" => "Power cycles",
        "unsafe_shutdowns" => "Unsafe shutdowns",
        "media_errors" => "Media errors",
        "error_log_entries" => "Error-log entries",
        "data_units_read" => "Data units read",
        "data_units_written" => "Data units written",
        "host_read_commands" => "Host read commands",
        "host_write_commands" => "Host write commands",
        "warning_temperature_time" => "Warning temperature time",
        "critical_temperature_time" => "Critical temperature time",
        _ => key,
    }
}

pub fn hardware_property_label(key: &str) -> String {
    if is_storage_health_property(key) {
        return storage_health_property_label(key).to_owned();
    }
    match key {
        "os_pretty_name" => "Operating system".to_owned(),
        "os_id" => "Distribution ID".to_owned(),
        "kernel_release" => "Kernel".to_owned(),
        "kernel_version" => "Kernel build".to_owned(),
        "board_name" => "Board name".to_owned(),
        "board_version" => "Board revision".to_owned(),
        "bios_vendor" => "BIOS vendor".to_owned(),
        "bios_version" => "BIOS version".to_owned(),
        "bios_date" => "BIOS release date".to_owned(),
        "release_date" => "Release date".to_owned(),
        "bios_release" => "Platform firmware revision".to_owned(),
        "ec_firmware_release" => "EC firmware release".to_owned(),
        "agesa_version" => "AGESA version".to_owned(),
        "smbios_version" => "SMBIOS version".to_owned(),
        "platform_firmware_revision" => "Platform firmware revision".to_owned(),
        "uefi_supported" => "UEFI support".to_owned(),
        "firmware_boot_status" => "Firmware boot status".to_owned(),
        "firmware_upgradeable" => "Firmware upgradeable".to_owned(),
        "rom_size" => "Firmware ROM size".to_owned(),
        "management_device" => "Management controller".to_owned(),
        "management_device_type" => "Controller type".to_owned(),
        "socket_designation" => "Socket".to_owned(),
        "processor_upgrade" => "Socket type".to_owned(),
        "firmware_maximum_frequency_hz" => "Firmware maximum frequency".to_owned(),
        "installed_capacity_bytes" => "Installed capacity".to_owned(),
        "maximum_capacity_bytes" => "Maximum capacity".to_owned(),
        "memory_slots" => "Slots populated".to_owned(),
        "memory_slots_populated" => "Populated slots".to_owned(),
        "memory_slots_total" => "Total slots".to_owned(),
        "error_correction_type" => "Error correction".to_owned(),
        "slot_label" | "locator" => "Slot".to_owned(),
        "bank_locator" => "Bank / channel".to_owned(),
        "form_factor" => "Form factor".to_owned(),
        "module_type" => "Module type".to_owned(),
        "spd_speed" => "SPD/default speed".to_owned(),
        "configured_memory_speed" => "Configured speed".to_owned(),
        "minimum_voltage" => "Minimum voltage".to_owned(),
        "maximum_voltage" => "Maximum voltage".to_owned(),
        "configured_voltage" => "Configured voltage".to_owned(),
        "memory_technology" => "Memory technology".to_owned(),
        "memory_operating_mode" => "Operating mode".to_owned(),
        "capacity_bytes" => "Capacity".to_owned(),
        "total_bytes" => "Usable capacity".to_owned(),
        "tpm_specification" => "Specification".to_owned(),
        "tpm_firmware_revision" => "Firmware revision".to_owned(),
        "tpm_firmware_version" => "Firmware version".to_owned(),
        "tpm_firmware_release_date" => "Firmware release date".to_owned(),
        "tpm_firmware_state" => "Firmware state".to_owned(),
        "tpm_firmware_updatable" => "Firmware updatable".to_owned(),
        "tpm_firmware_write_protected" => "Firmware write-protected".to_owned(),
        "tpm_characteristics" => "Characteristics".to_owned(),
        "mac_address" => "MAC address".to_owned(),
        "speed_mbps" => "Link speed".to_owned(),
        "current_link_speed" => "Current link speed".to_owned(),
        "maximum_link_speed" | "max_link_speed" => "Maximum link speed".to_owned(),
        "current_link_width" => "Current link width".to_owned(),
        "maximum_link_width" | "max_link_width" => "Maximum link width".to_owned(),
        _ => humanize_key(key),
    }
}

pub fn is_low_level_hardware_property(key: &str) -> bool {
    key.starts_with("vulnerability_")
        || key.ends_with("_url")
        || matches!(
            key,
            "flags"
                | "os_ansi_color"
                | "os_logo"
                | "os_build_id"
                | "resource_table_bytes"
                | "hwmon_name"
                | "hwmon_driver"
                | "update_interval_us"
                | "policies_sampled"
                | "inventory_source"
                | "memory_role"
                | "dmi_record_index"
                | "telemetry_mapping"
                | "telemetry_bus_address"
                | "minimum_voltage"
                | "maximum_voltage"
                | "configured_voltage"
                | "sensor_family"
                | "gpu_metrics_bytes_read"
                | "gpu_metrics_structure_size"
                | "gpu_metrics_format_revision"
                | "gpu_metrics_content_revision"
                | "ethtool_supports_eeprom_access"
                | "ethtool_supports_priv_flags"
                | "ethtool_supports_register_dump"
                | "ethtool_supports_statistics"
                | "ethtool_supports_test"
        )
}

pub fn escape_delimited(value: &str, delimiter: char) -> String {
    if value
        .chars()
        .any(|character| character == delimiter || matches!(character, '"' | '\n' | '\r'))
    {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

pub fn sensor_kind_name(kind: SensorKind) -> &'static str {
    match kind {
        SensorKind::Temperature => "Temperatures",
        SensorKind::Voltage => "Voltages",
        SensorKind::Current => "Current",
        SensorKind::Power => "Power",
        SensorKind::Energy => "Energy",
        SensorKind::Fan => "Fans",
        SensorKind::Frequency => "Clocks",
        SensorKind::Throughput => "Throughput",
        SensorKind::Utilization => "Utilization",
        SensorKind::Capacity => "Capacity",
        SensorKind::Humidity => "Humidity",
        SensorKind::Counter => "Counters",
        SensorKind::Boolean => "Status",
        SensorKind::Other => "Other readings",
    }
}

pub fn property_to_string(value: &PropertyValue) -> String {
    match value {
        PropertyValue::String(value) => value.clone(),
        PropertyValue::Integer(value) => value.to_string(),
        PropertyValue::Unsigned(value) => value.to_string(),
        PropertyValue::Float(value) => format_float(*value),
        PropertyValue::Boolean(value) => {
            if *value {
                "Yes".to_owned()
            } else {
                "No".to_owned()
            }
        }
        PropertyValue::Strings(values) => values.join(", "),
    }
}

pub fn format_property_value(key: &str, value: &PropertyValue) -> Option<String> {
    let raw = property_to_string(value);
    if is_placeholder(&raw) {
        return None;
    }

    if key == "size_mb" {
        return numeric_value(value).map(|value| format_bytes(value * 1024.0 * 1024.0));
    }
    if key.ends_with("_bytes")
        || matches!(
            key,
            "logical_block_size" | "physical_block_size" | "minimum_io_size" | "optimal_io_size"
        )
    {
        return numeric_value(value).map(format_bytes);
    }
    if key.ends_with("_frequency_hz") || key.ends_with("_freq_hz") {
        return numeric_value(value).map(format_frequency);
    }
    if key == "speed_mbps" {
        return numeric_value(value).map(format_network_speed);
    }
    if matches!(key, "configured_memory_speed" | "spd_speed") {
        return Some(if raw.contains("MT/s") || raw.contains("MHz") {
            raw
        } else {
            format!("{raw} MT/s")
        });
    }
    if key == "configured_voltage" {
        return Some(if raw.contains('V') {
            raw
        } else {
            format!("{raw} V")
        });
    }
    if key == "current_link_width" || key == "maximum_link_width" {
        return Some(if raw.starts_with('x') {
            raw
        } else {
            format!("x{raw}")
        });
    }
    if key == "critical_warning" {
        return Some(match numeric_value(value) {
            Some(0.0) => "None".to_owned(),
            Some(bits) => format!("0x{:02x}", bits as u64),
            None => raw,
        });
    }
    if matches!(
        key,
        "percentage_used" | "available_spare" | "spare_threshold" | "capacity"
    ) {
        return numeric_value(value).map(|value| format!("{value:.0} %"));
    }
    if matches!(
        key,
        "warning_temperature_time" | "critical_temperature_time"
    ) {
        return numeric_value(value).map(|value| format!("{value:.0} min"));
    }
    if key == "update_interval_ms" {
        return numeric_value(value).map(|value| format!("{value:.0} ms"));
    }
    Some(raw)
}

pub(super) fn numeric_value(value: &PropertyValue) -> Option<f64> {
    value.as_f64()
}

pub fn format_value(value: f64, unit: &Unit) -> String {
    match unit {
        Unit::Celsius => format!("{value:.1} °C"),
        Unit::Volt => format!("{value:.3} V"),
        Unit::Ampere => format!("{value:.3} A"),
        Unit::Watt => format!("{value:.2} W"),
        Unit::WattHour => format!("{value:.2} Wh"),
        Unit::AmpereHour => format!("{value:.2} Ah"),
        Unit::Joule => format!("{value:.2} J"),
        Unit::Rpm => format!("{value:.0} RPM"),
        Unit::Hertz => format_frequency(value),
        Unit::Percent => format!("{value:.1} %"),
        Unit::Byte => format_bytes(value),
        Unit::BytePerSecond => format!("{}/s", format_bytes(value)),
        Unit::Count => format!("{value:.0}"),
        Unit::CountPerSecond => format!("{value:.1}/s"),
        Unit::Boolean => {
            if value != 0.0 {
                "Yes".to_owned()
            } else {
                "No".to_owned()
            }
        }
        Unit::Raw => format_float(value),
    }
}

pub(super) fn format_frequency(value: f64) -> String {
    if value >= 1_000_000_000.0 {
        format!("{:.3} GHz", value / 1_000_000_000.0)
    } else if value >= 1_000_000.0 {
        format!("{:.1} MHz", value / 1_000_000.0)
    } else if value >= 1_000.0 {
        format!("{:.1} kHz", value / 1_000.0)
    } else {
        format!("{value:.0} Hz")
    }
}

pub(super) fn format_bytes(value: f64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut value = value.max(0.0);
    let mut index = 0;
    while value >= 1024.0 && index + 1 < UNITS.len() {
        value /= 1024.0;
        index += 1;
    }
    if index == 0 {
        format!("{value:.0} {}", UNITS[index])
    } else {
        format!("{value:.2} {}", UNITS[index])
    }
}

pub(super) fn format_network_speed(mbps: f64) -> String {
    if mbps >= 1000.0 {
        format!("{:.1} Gb/s", mbps / 1000.0)
    } else {
        format!("{mbps:.0} Mb/s")
    }
}

pub fn humanize_key(key: &str) -> String {
    let mut words = Vec::new();
    for token in key.split('_') {
        let word = match token {
            "cpu" => "CPU".to_owned(),
            "gpu" => "GPU".to_owned(),
            "usb" => "USB".to_owned(),
            "pci" => "PCI".to_owned(),
            "pcie" => "PCIe".to_owned(),
            "nvme" => "NVMe".to_owned(),
            "bios" => "BIOS".to_owned(),
            "uefi" => "UEFI".to_owned(),
            "uuid" => "UUID".to_owned(),
            "wwid" => "WWID".to_owned(),
            "iommu" => "IOMMU".to_owned(),
            "mtu" => "MTU".to_owned(),
            "rx" => "RX".to_owned(),
            "tx" => "TX".to_owned(),
            "id" => "ID".to_owned(),
            "io" => "I/O".to_owned(),
            other => other.to_owned(),
        };
        words.push(word);
    }
    let mut label = words.join(" ");
    if let Some(first) = label.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    label
}

pub(super) fn is_placeholder(value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    value.is_empty()
        || matches!(
            value.as_str(),
            "null"
                | "none"
                | "unknown"
                | "not specified"
                | "default string"
                | "system product name"
                | "system version"
                | "to be filled by o.e.m."
        )
}

fn format_float(value: f64) -> String {
    if value.fract().abs() < f64::EPSILON {
        format!("{value:.0}")
    } else {
        format!("{value:.3}")
    }
}
