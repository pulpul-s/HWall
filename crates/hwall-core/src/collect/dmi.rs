use super::memory::{locator_is_specific, slot_device_id};
use super::util::{add_string, command_path, read_trimmed, run_command_configured, HELPER_TIMEOUT};
use crate::model::{Device, DeviceClass, PropertyValue, SnapshotBuilder};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

pub(super) fn collect(
    builder: &mut SnapshotBuilder,
    allow_helper_commands: bool,
    include_sensitive: bool,
) {
    collect_sysfs(builder, include_sensitive);
    if allow_helper_commands {
        collect_dmidecode(builder, include_sensitive);
    }
}

fn collect_sysfs(builder: &mut SnapshotBuilder, include_sensitive: bool) {
    let root = Path::new("/sys/class/dmi/id");
    if !root.exists() {
        return;
    }

    let board_vendor = read_clean(root.join("board_vendor"));
    let board_name = read_clean(root.join("board_name"));
    let product_name = read_clean(root.join("product_name"));
    let name = match (&board_vendor, &board_name, &product_name) {
        (Some(vendor), Some(board), _) => format!("{vendor} {board}"),
        (_, Some(board), _) => board.clone(),
        (_, _, Some(product)) => product.clone(),
        _ => "Motherboard".to_owned(),
    };

    let mut board = Device::new("motherboard:0", DeviceClass::Motherboard, name);
    board.vendor = board_vendor;
    board.model = board_name;
    add_string(
        &mut board.properties,
        "board_version",
        read_clean(root.join("board_version")),
    );
    add_string(&mut board.properties, "product_name", product_name);
    add_string(
        &mut board.properties,
        "product_version",
        read_clean(root.join("product_version")),
    );
    add_string(
        &mut board.properties,
        "chassis_vendor",
        read_clean(root.join("chassis_vendor")),
    );
    add_string(
        &mut board.properties,
        "chassis_type",
        read_clean(root.join("chassis_type")),
    );
    if include_sensitive {
        add_string(
            &mut board.properties,
            "board_serial",
            read_clean(root.join("board_serial")),
        );
        add_string(
            &mut board.properties,
            "product_serial",
            read_clean(root.join("product_serial")),
        );
        add_string(
            &mut board.properties,
            "product_uuid",
            read_clean(root.join("product_uuid")),
        );
        add_string(
            &mut board.properties,
            "chassis_serial",
            read_clean(root.join("chassis_serial")),
        );
    }
    builder.add_device(board);

    let bios_vendor = read_clean(root.join("bios_vendor"));
    let bios_version = read_clean(root.join("bios_version"));
    let bios_date = read_clean(root.join("bios_date"));
    let bios_release = read_clean(root.join("bios_release"));
    let ec_firmware_release = read_clean(root.join("ec_firmware_release"));
    if bios_vendor.is_some()
        || bios_version.is_some()
        || bios_date.is_some()
        || bios_release.is_some()
        || ec_firmware_release.is_some()
    {
        let mut bios = Device::new("bios:0", DeviceClass::Bios, "System firmware");
        bios.vendor = bios_vendor;
        bios.model = bios_version;
        bios.parent = Some("motherboard:0".to_owned());
        add_string(&mut bios.properties, "release_date", bios_date);
        add_string(&mut bios.properties, "bios_release", bios_release);
        add_string(
            &mut bios.properties,
            "ec_firmware_release",
            ec_firmware_release,
        );
        builder.add_device(bios);
    }
}

fn collect_dmidecode(builder: &mut SnapshotBuilder, include_sensitive: bool) {
    let Some(program) = command_path("dmidecode") else {
        return;
    };

    let mut command = Command::new(program);
    command.env("LC_ALL", "C");
    let Ok(output) = run_command_configured(&mut command, HELPER_TIMEOUT) else {
        return;
    };
    if !output.status.success() {
        return;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    collect_dmidecode_text(builder, &text, include_sensitive);
}

fn collect_dmidecode_text(builder: &mut SnapshotBuilder, text: &str, include_sensitive: bool) {
    if let Some(version) = parse_smbios_version(text) {
        let mut board = Device::new("motherboard:0", DeviceClass::Motherboard, "Motherboard");
        board
            .properties
            .insert("smbios_version".to_owned(), version.into());
        builder.add_device(board);
    }

    let records = parse_records(text);
    let total_memory_slots = records.iter().find_map(system_memory_slot_count);
    let mut installed_memory = 0_u64;
    let mut populated_slots = 0_u64;

    for (index, record) in records.iter().enumerate() {
        match record.type_id {
            0 => collect_firmware_record(builder, record),
            1 => collect_system_record(builder, record),
            2 => collect_baseboard_record(builder, record),
            3 => collect_chassis_record(builder, record),
            4 => collect_processor_record(builder, record),
            16 => collect_memory_array_record(builder, record),
            17 => {
                if let Some(capacity) =
                    collect_memory_device_record(builder, record, index, include_sensitive)
                {
                    installed_memory = installed_memory.saturating_add(capacity);
                    populated_slots = populated_slots.saturating_add(1);
                }
            }
            32 => collect_boot_information_record(builder, record),
            34 => collect_management_device_record(builder, record),
            40 => collect_additional_information_record(builder, record),
            43 => collect_tpm_record(builder, record),
            45 => collect_firmware_inventory_record(builder, record),
            _ => {}
        }
    }

    if installed_memory > 0 || populated_slots > 0 {
        let mut memory = Device::new("memory:system", DeviceClass::Memory, "System memory");
        memory.properties.insert(
            "installed_capacity_bytes".to_owned(),
            installed_memory.into(),
        );
        memory
            .properties
            .insert("memory_slots_populated".to_owned(), populated_slots.into());
        if let Some(total) = total_memory_slots {
            memory.properties.insert(
                "memory_slots".to_owned(),
                format!("{populated_slots} of {total}").into(),
            );
        }
        builder.add_device(memory);
    }
}

fn system_memory_slot_count(record: &DmiRecord) -> Option<u64> {
    if record.type_id != 16 {
        return None;
    }
    let fields = record.fields();
    if raw_field(&fields, "use").is_some_and(|value| !value.eq_ignore_ascii_case("System Memory")) {
        return None;
    }
    raw_field(&fields, "number_of_devices")?.parse().ok()
}

fn collect_firmware_record(builder: &mut SnapshotBuilder, record: &DmiRecord) {
    let fields = record.fields();
    let mut firmware = Device::new("bios:0", DeviceClass::Bios, "System firmware");
    firmware.parent = Some("motherboard:0".to_owned());
    firmware.vendor = valid_field(&fields, "vendor");
    firmware.model = valid_field(&fields, "version");
    add_field(
        &mut firmware.properties,
        "release_date",
        &fields,
        "release_date",
    );
    add_field(
        &mut firmware.properties,
        "platform_firmware_revision",
        &fields,
        "platform_firmware_revision",
    );
    add_field(&mut firmware.properties, "rom_size", &fields, "rom_size");

    let characteristics = record.list_after("Characteristics:");
    if characteristics
        .iter()
        .any(|value| value.eq_ignore_ascii_case("UEFI is supported"))
    {
        firmware
            .properties
            .insert("uefi_supported".to_owned(), true.into());
    }
    if characteristics.iter().any(|value| {
        value.eq_ignore_ascii_case("Firmware is upgradeable")
            || value.eq_ignore_ascii_case("BIOS is upgradeable")
    }) {
        firmware
            .properties
            .insert("firmware_upgradeable".to_owned(), true.into());
    }
    builder.add_device(firmware);
}

fn collect_system_record(builder: &mut SnapshotBuilder, record: &DmiRecord) {
    let fields = record.fields();
    let mut board = Device::new("motherboard:0", DeviceClass::Motherboard, "Motherboard");
    add_field(
        &mut board.properties,
        "system_manufacturer",
        &fields,
        "manufacturer",
    );
    add_field(
        &mut board.properties,
        "product_name",
        &fields,
        "product_name",
    );
    add_field(&mut board.properties, "product_version", &fields, "version");
    add_field(&mut board.properties, "system_family", &fields, "family");
    builder.add_device(board);
}

fn collect_baseboard_record(builder: &mut SnapshotBuilder, record: &DmiRecord) {
    let fields = record.fields();
    let mut board = Device::new("motherboard:0", DeviceClass::Motherboard, "Motherboard");
    board.vendor = valid_field(&fields, "manufacturer");
    board.model = valid_field(&fields, "product_name");
    add_field(&mut board.properties, "board_version", &fields, "version");
    add_field(&mut board.properties, "board_type", &fields, "type");
    builder.add_device(board);
}

fn collect_chassis_record(builder: &mut SnapshotBuilder, record: &DmiRecord) {
    let fields = record.fields();
    let mut board = Device::new("motherboard:0", DeviceClass::Motherboard, "Motherboard");
    add_field(
        &mut board.properties,
        "chassis_form_factor",
        &fields,
        "type",
    );
    builder.add_device(board);
}

fn collect_processor_record(builder: &mut SnapshotBuilder, record: &DmiRecord) {
    let fields = record.fields();
    let model = valid_field(&fields, "version");
    let name = model.as_deref().unwrap_or("CPU").to_owned();
    let mut cpu = Device::new("cpu:0", DeviceClass::Cpu, name);
    cpu.vendor = valid_field(&fields, "manufacturer");
    cpu.model = model;
    let socket = valid_field(&fields, "socket_designation");
    if let Some(socket) = socket.as_deref() {
        cpu.properties
            .insert("socket_designation".to_owned(), socket.into());
    }
    if let Some(upgrade) = valid_field(&fields, "upgrade") {
        let normalized_upgrade = normalize_socket_name(&upgrade);
        let duplicate = socket
            .as_deref()
            .is_some_and(|socket| normalize_socket_name(socket) == normalized_upgrade);
        if !duplicate {
            cpu.properties
                .insert("processor_upgrade".to_owned(), upgrade.into());
        }
    }
    add_integer_field(
        &mut cpu.properties,
        "firmware_core_count",
        &fields,
        "core_count",
    );
    add_integer_field(
        &mut cpu.properties,
        "firmware_core_enabled",
        &fields,
        "core_enabled",
    );
    add_integer_field(
        &mut cpu.properties,
        "firmware_thread_count",
        &fields,
        "thread_count",
    );
    add_integer_field(
        &mut cpu.properties,
        "firmware_thread_enabled",
        &fields,
        "thread_enabled",
    );
    if let Some(value) = fields
        .get("max_speed")
        .and_then(|value| parse_frequency_hz(value))
    {
        cpu.properties
            .insert("firmware_maximum_frequency_hz".to_owned(), value.into());
    }
    builder.add_device(cpu);
}

fn collect_memory_array_record(builder: &mut SnapshotBuilder, record: &DmiRecord) {
    let fields = record.fields();
    if raw_field(&fields, "use").is_some_and(|value| !value.eq_ignore_ascii_case("System Memory")) {
        return;
    }

    let mut memory = Device::new("memory:system", DeviceClass::Memory, "System memory");
    if let Some(capacity) = raw_field(&fields, "maximum_capacity").and_then(parse_capacity_bytes) {
        memory
            .properties
            .insert("maximum_capacity_bytes".to_owned(), capacity.into());
    }
    add_integer_field(
        &mut memory.properties,
        "memory_slots_total",
        &fields,
        "number_of_devices",
    );
    if let Some(value) = field_allow_none(&fields, "error_correction_type") {
        memory
            .properties
            .insert("error_correction_type".to_owned(), value.into());
    }
    builder.add_device(memory);
}

fn collect_memory_device_record(
    builder: &mut SnapshotBuilder,
    record: &DmiRecord,
    index: usize,
    include_sensitive: bool,
) -> Option<u64> {
    let fields = record.fields();
    let size = raw_field(&fields, "size")?;
    if size.eq_ignore_ascii_case("No Module Installed") {
        return None;
    }
    let capacity = parse_capacity_bytes(size)?;

    let locator = valid_field(&fields, "locator");
    let bank = valid_field(&fields, "bank_locator");
    let slot_label = memory_slot_label(locator.as_deref(), bank.as_deref());
    let manufacturer = valid_field(&fields, "manufacturer");
    let part_number = valid_field(&fields, "part_number");
    let display_name = match (&manufacturer, &part_number, &slot_label) {
        (Some(vendor), Some(part), _) => format!("{vendor} {part}"),
        (_, _, Some(slot)) => format!("Memory module {slot}"),
        _ => format!("Memory module {}", index + 1),
    };
    let device_id = slot_label
        .as_deref()
        .and_then(slot_device_id)
        .unwrap_or_else(|| format!("memory:dmi:{index}"));

    let mut device = Device::new(device_id, DeviceClass::Memory, display_name);
    device.vendor = manufacturer;
    device.model = part_number;
    device
        .properties
        .insert("memory_role".to_owned(), "module".into());
    device
        .properties
        .insert("inventory_source".to_owned(), "dmidecode".into());
    device
        .properties
        .insert("dmi_record_index".to_owned(), (index as u64).into());
    device
        .properties
        .insert("capacity_bytes".to_owned(), capacity.into());
    if let Some(slot_label) = slot_label {
        device
            .properties
            .insert("slot_label".to_owned(), slot_label.into());
    }
    if let Some(locator) = locator {
        device
            .properties
            .insert("locator".to_owned(), locator.into());
    }
    if let Some(bank) = bank {
        device
            .properties
            .insert("bank_locator".to_owned(), bank.into());
    }

    for (property, field) in [
        ("form_factor", "form_factor"),
        ("memory_type", "type"),
        ("module_type", "type_detail"),
        ("spd_speed", "speed"),
        ("configured_memory_speed", "configured_memory_speed"),
        ("rank", "rank"),
        ("data_width", "data_width"),
        ("total_width", "total_width"),
        ("minimum_voltage", "minimum_voltage"),
        ("maximum_voltage", "maximum_voltage"),
        ("configured_voltage", "configured_voltage"),
        ("memory_technology", "memory_technology"),
        ("memory_operating_mode", "memory_operating_mode_capability"),
    ] {
        add_field(&mut device.properties, property, &fields, field);
    }
    if include_sensitive {
        add_field(&mut device.properties, "serial", &fields, "serial_number");
    }
    builder.add_device(device);
    Some(capacity)
}

fn collect_boot_information_record(builder: &mut SnapshotBuilder, record: &DmiRecord) {
    let fields = record.fields();
    let mut firmware = Device::new("bios:0", DeviceClass::Bios, "System firmware");
    firmware.parent = Some("motherboard:0".to_owned());
    add_field(
        &mut firmware.properties,
        "firmware_boot_status",
        &fields,
        "status",
    );
    builder.add_device(firmware);
}

fn collect_management_device_record(builder: &mut SnapshotBuilder, record: &DmiRecord) {
    let fields = record.fields();
    let Some(description) = valid_field(&fields, "description") else {
        return;
    };
    let mut board = Device::new("motherboard:0", DeviceClass::Motherboard, "Motherboard");
    board
        .properties
        .insert("management_device".to_owned(), description.into());
    add_field(
        &mut board.properties,
        "management_device_type",
        &fields,
        "type",
    );
    add_field(
        &mut board.properties,
        "management_device_address",
        &fields,
        "address",
    );
    add_field(
        &mut board.properties,
        "management_device_address_type",
        &fields,
        "address_type",
    );
    builder.add_device(board);
}

fn collect_additional_information_record(builder: &mut SnapshotBuilder, record: &DmiRecord) {
    let Some(agesa) = record
        .values_named("String")
        .into_iter()
        .find_map(|value| parse_agesa(&value))
    else {
        return;
    };
    let mut board = Device::new("motherboard:0", DeviceClass::Motherboard, "Motherboard");
    board
        .properties
        .insert("agesa_version".to_owned(), agesa.into());
    builder.add_device(board);
}

fn collect_tpm_record(builder: &mut SnapshotBuilder, record: &DmiRecord) {
    let fields = record.fields();
    let mut tpm = Device::new(
        "security:tpm:0",
        DeviceClass::Other,
        "Trusted Platform Module",
    );
    tpm.parent = Some("motherboard:0".to_owned());
    let vendor = valid_field(&fields, "vendor_id");
    let description = valid_field(&fields, "description");
    tpm.vendor = vendor.clone();
    if description.as_deref() != vendor.as_deref() {
        tpm.model = description;
    }
    tpm.properties
        .insert("security_role".to_owned(), "tpm".into());
    add_field(
        &mut tpm.properties,
        "tpm_specification",
        &fields,
        "specification_version",
    );
    add_field(
        &mut tpm.properties,
        "tpm_firmware_revision",
        &fields,
        "firmware_revision",
    );
    let characteristics = record.list_after("Characteristics:");
    if !characteristics.is_empty() {
        tpm.properties.insert(
            "tpm_characteristics".to_owned(),
            PropertyValue::Strings(characteristics),
        );
    }
    builder.add_device(tpm);
}

fn collect_firmware_inventory_record(builder: &mut SnapshotBuilder, record: &DmiRecord) {
    let fields = record.fields();
    let Some(component) = valid_field(&fields, "firmware_component_name") else {
        return;
    };
    if !component.to_ascii_lowercase().contains("tpm") {
        return;
    }

    let mut tpm = Device::new(
        "security:tpm:0",
        DeviceClass::Other,
        "Trusted Platform Module",
    );
    tpm.parent = Some("motherboard:0".to_owned());
    tpm.properties
        .insert("security_role".to_owned(), "tpm".into());
    add_field(
        &mut tpm.properties,
        "tpm_firmware_version",
        &fields,
        "firmware_version",
    );
    add_field(
        &mut tpm.properties,
        "tpm_firmware_release_date",
        &fields,
        "release_date",
    );
    add_field(&mut tpm.properties, "tpm_firmware_state", &fields, "state");
    add_bool_field(
        &mut tpm.properties,
        "tpm_firmware_updatable",
        &fields,
        "updatable",
    );
    add_bool_field(
        &mut tpm.properties,
        "tpm_firmware_write_protected",
        &fields,
        "write_protect",
    );
    if tpm.vendor.is_none() {
        tpm.vendor = valid_field(&fields, "manufacturer");
    }
    builder.add_device(tpm);
}

#[derive(Debug)]
struct DmiRecord {
    type_id: u32,
    body: String,
}

impl DmiRecord {
    fn fields(&self) -> BTreeMap<String, String> {
        parse_fields(&self.body)
    }

    fn values_named(&self, name: &str) -> Vec<String> {
        self.body
            .lines()
            .filter_map(|line| line.trim().split_once(':'))
            .filter(|(key, _)| key.trim().eq_ignore_ascii_case(name))
            .map(|(_, value)| value.trim().to_owned())
            .filter(|value| is_useful_value(value))
            .collect()
    }

    fn list_after(&self, heading: &str) -> Vec<String> {
        let mut values = Vec::new();
        let mut collecting = false;
        for line in self.body.lines() {
            let trimmed = line.trim();
            if trimmed == heading {
                collecting = true;
                continue;
            }
            if !collecting {
                continue;
            }
            if trimmed.is_empty() || trimmed.contains(':') {
                break;
            }
            if is_useful_value(trimmed) {
                values.push(trimmed.to_owned());
            }
        }
        values
    }
}

fn parse_records(text: &str) -> Vec<DmiRecord> {
    text.split("Handle ")
        .skip(1)
        .filter_map(|chunk| {
            let mut lines = chunk.lines();
            let header = lines.next()?;
            let type_text = header.split("DMI type ").nth(1)?.split(',').next()?;
            let type_id = type_text.trim().parse().ok()?;
            let body = lines.collect::<Vec<_>>().join("\n");
            Some(DmiRecord { type_id, body })
        })
        .collect()
}

fn parse_fields(block: &str) -> BTreeMap<String, String> {
    block
        .lines()
        .filter_map(|line| line.trim().split_once(':'))
        .map(|(key, value)| {
            (
                key.trim().to_ascii_lowercase().replace([' ', '-'], "_"),
                value.trim().to_owned(),
            )
        })
        .filter(|(_, value)| !value.is_empty())
        .collect()
}

fn raw_field<'a>(fields: &'a BTreeMap<String, String>, key: &str) -> Option<&'a str> {
    fields.get(key).map(String::as_str).map(str::trim)
}

fn valid_field(fields: &BTreeMap<String, String>, key: &str) -> Option<String> {
    raw_field(fields, key)
        .filter(|value| is_useful_value(value))
        .map(ToOwned::to_owned)
}

fn field_allow_none(fields: &BTreeMap<String, String>, key: &str) -> Option<String> {
    raw_field(fields, key)
        .filter(|value| !is_placeholder(value, false))
        .map(ToOwned::to_owned)
}

fn read_clean(path: impl AsRef<Path>) -> Option<String> {
    read_trimmed(path).filter(|value| is_useful_value(value))
}

fn is_useful_value(value: &str) -> bool {
    !is_placeholder(value, true)
}

fn is_placeholder(value: &str, reject_none: bool) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    normalized.is_empty()
        || (reject_none && normalized == "none")
        || matches!(
            normalized.as_str(),
            "unknown"
                | "not specified"
                | "not installed"
                | "default string"
                | "to be filled by o.e.m."
                | "to be filled by oem"
                | "system product name"
                | "system version"
                | "system serial number"
                | "sku"
        )
}

fn add_field(
    properties: &mut BTreeMap<String, PropertyValue>,
    property: &str,
    fields: &BTreeMap<String, String>,
    field: &str,
) {
    if let Some(value) = valid_field(fields, field) {
        properties.insert(property.to_owned(), value.into());
    }
}

fn add_integer_field(
    properties: &mut BTreeMap<String, PropertyValue>,
    property: &str,
    fields: &BTreeMap<String, String>,
    field: &str,
) {
    if let Some(value) = raw_field(fields, field).and_then(|value| value.parse::<u64>().ok()) {
        properties.insert(property.to_owned(), value.into());
    }
}

fn add_bool_field(
    properties: &mut BTreeMap<String, PropertyValue>,
    property: &str,
    fields: &BTreeMap<String, String>,
    field: &str,
) {
    let value = raw_field(fields, field).and_then(parse_bool);
    if let Some(value) = value {
        properties.insert(property.to_owned(), value.into());
    }
}

fn memory_slot_label(locator: Option<&str>, bank: Option<&str>) -> Option<String> {
    match (locator, bank) {
        (Some(locator), Some(bank)) if locator_is_specific(bank) => {
            Some(format!("{bank} / {locator}"))
        }
        (Some(locator), _) if locator_is_specific(locator) => Some(locator.to_owned()),
        (_, Some(bank)) if locator_is_specific(bank) => Some(bank.to_owned()),
        _ => None,
    }
}

fn normalize_socket_name(value: &str) -> String {
    let normalized = value.trim().to_ascii_lowercase();
    normalized
        .strip_prefix("socket ")
        .unwrap_or(&normalized)
        .to_owned()
}

fn parse_capacity_bytes(value: &str) -> Option<u64> {
    let mut parts = value.split_whitespace();
    let amount = parts.next()?.parse::<f64>().ok()?;
    let unit = parts.next()?.to_ascii_lowercase();
    let multiplier = match unit.as_str() {
        "b" | "bytes" => 1.0,
        "kb" | "kib" => 1024.0,
        "mb" | "mib" => 1024.0 * 1024.0,
        "gb" | "gib" => 1024.0 * 1024.0 * 1024.0,
        "tb" | "tib" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    let bytes = amount * multiplier;
    (bytes.is_finite() && bytes >= 0.0 && bytes <= u64::MAX as f64).then_some(bytes as u64)
}

fn parse_frequency_hz(value: &str) -> Option<f64> {
    let mut parts = value.split_whitespace();
    let amount = parts.next()?.parse::<f64>().ok()?;
    let multiplier = match parts.next()?.to_ascii_lowercase().as_str() {
        "hz" => 1.0,
        "khz" => 1_000.0,
        "mhz" => 1_000_000.0,
        "ghz" => 1_000_000_000.0,
        _ => return None,
    };
    let hertz = amount * multiplier;
    (hertz.is_finite() && hertz >= 0.0).then_some(hertz)
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "yes" | "true" | "enabled" | "supported" => Some(true),
        "no" | "false" | "disabled" | "not supported" => Some(false),
        _ => None,
    }
}

fn parse_agesa(value: &str) -> Option<String> {
    let position = value.to_ascii_lowercase().find("agesa")?;
    let mut remainder = value[position + "agesa".len()..].trim();
    if remainder.starts_with('!') {
        remainder = remainder
            .split_once(char::is_whitespace)
            .map(|(_, value)| value.trim())
            .unwrap_or_default();
    }
    (!remainder.is_empty()).then(|| remainder.to_owned())
}

fn parse_smbios_version(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let version = line
            .trim()
            .strip_prefix("SMBIOS ")?
            .strip_suffix(" present.")?;
        (!version.trim().is_empty()).then(|| version.trim().to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"# dmidecode 3.7
SMBIOS 3.6.0 present.

Handle 0x0000, DMI type 0, 26 bytes
Platform Firmware Information
        Vendor: American Megatrends Inc.
        Version: 3854
        Release Date: 04/03/2026
        Platform Firmware Revision: 38.54
        Characteristics:
                Firmware is upgradeable
                UEFI is supported

Handle 0x0010, DMI type 4, 50 bytes
Processor Information
        Socket Designation: AM5
        Upgrade: Socket AM5
        Core Count: 8
        Thread Count: 16
        Max Speed: 5650 MHz

Handle 0x0008, DMI type 34, 11 bytes
Management Device
        Description: Nuvoton NCT6799D-R
        Type: Other
        Address: 0x00000295
        Address Type: I/O Port

Handle 0x0013, DMI type 16, 23 bytes
Physical Memory Array
        Use: System Memory
        Error Correction Type: None
        Maximum Capacity: 256 GiB
        Number Of Devices: 4

Handle 0x0018, DMI type 17, 92 bytes
Memory Device
        Size: 32 GiB
        Form Factor: DIMM
        Locator: DIMM 1
        Bank Locator: P0 CHANNEL A
        Type: DDR5
        Type Detail: Synchronous Unbuffered (Unregistered)
        Speed: 4800 MT/s
        Manufacturer: Kingston
        Serial Number: EXAMPLE
        Part Number: KF556C40-32
        Rank: 2
        Configured Memory Speed: 6000 MT/s
        Memory Technology: DRAM

Handle 0x001F, DMI type 40, 14 bytes
Additional Information 1
        String: AGESA!V9 ComboAm5PI 1.3.0.1

Handle 0x000B, DMI type 43, 31 bytes
TPM Device
        Vendor ID: AMD
        Specification Version: 2.0
        Firmware Revision: 6.32
        Description: AMD

Handle 0x000C, DMI type 45, 26 bytes
Firmware Inventory Information
        Firmware Component Name: TPM Firmware
        Firmware Version: 60020.6
        Release Date: 2021-05-15T00:00:00Z
        Manufacturer: AMD
        State: Enabled
        Characteristics:
                Updatable: No
                Write-Protect: No
"#;

    #[test]
    fn parses_relevant_dmi_records() {
        let records = parse_records(SAMPLE);
        assert_eq!(records.len(), 8);
        assert_eq!(records[3].type_id, 16);
        assert_eq!(
            records[4].fields().get("part_number").map(String::as_str),
            Some("KF556C40-32")
        );
        assert_eq!(parse_smbios_version(SAMPLE).as_deref(), Some("3.6.0"));
    }

    #[test]
    fn projects_selected_dmi_information_into_devices() {
        let mut builder = SnapshotBuilder::default();
        collect_dmidecode_text(&mut builder, SAMPLE, false);
        let snapshot = builder.finish();

        let board = snapshot
            .devices
            .iter()
            .find(|device| device.id == "motherboard:0")
            .unwrap();
        assert_eq!(board.property_str("smbios_version"), Some("3.6.0"));
        assert_eq!(
            board.property_str("agesa_version"),
            Some("ComboAm5PI 1.3.0.1")
        );
        assert_eq!(
            board.property_str("management_device"),
            Some("Nuvoton NCT6799D-R")
        );

        let cpu = snapshot
            .devices
            .iter()
            .find(|device| device.id == "cpu:0")
            .unwrap();
        assert_eq!(cpu.property_str("socket_designation"), Some("AM5"));
        assert_eq!(cpu.property_str("processor_upgrade"), None);

        let memory = snapshot
            .devices
            .iter()
            .find(|device| device.id == "memory:system")
            .unwrap();
        assert_eq!(memory.property_str("memory_slots"), Some("1 of 4"));
        assert_eq!(
            memory
                .properties
                .get("maximum_capacity_bytes")
                .and_then(PropertyValue::as_u64),
            Some(256_u64 << 30)
        );

        let module = snapshot
            .devices
            .iter()
            .find(|device| device.property_str("memory_role") == Some("module"))
            .unwrap();
        assert_eq!(module.property_str("serial"), None);
        assert_eq!(module.property_str("spd_speed"), Some("4800 MT/s"));

        let tpm = snapshot
            .devices
            .iter()
            .find(|device| device.id == "security:tpm:0")
            .unwrap();
        assert_eq!(tpm.property_str("tpm_specification"), Some("2.0"));
        assert_eq!(tpm.property_str("tpm_firmware_version"), Some("60020.6"));
    }

    #[test]
    fn keeps_meaningful_none_but_filters_identity_placeholders() {
        let fields = parse_fields(concat!(
            "Error Correction Type: None\n",
            "Manufacturer: Default string\n",
            "Product Name: System Product Name",
        ));
        assert_eq!(
            field_allow_none(&fields, "error_correction_type").as_deref(),
            Some("None")
        );
        assert_eq!(valid_field(&fields, "manufacturer"), None);
        assert_eq!(valid_field(&fields, "product_name"), None);
    }

    #[test]
    fn combines_generic_locator_with_specific_bank() {
        assert_eq!(
            memory_slot_label(Some("DIMM 1"), Some("P0 CHANNEL A")).as_deref(),
            Some("P0 CHANNEL A / DIMM 1")
        );
    }

    #[test]
    fn parses_capacity_frequency_and_agesa() {
        assert_eq!(parse_capacity_bytes("32 GiB"), Some(34_359_738_368));
        assert_eq!(parse_frequency_hz("5650 MHz"), Some(5_650_000_000.0));
        assert_eq!(
            parse_agesa("AGESA!V9 ComboAm5PI 1.3.0.1").as_deref(),
            Some("ComboAm5PI 1.3.0.1")
        );
    }

    #[test]
    fn parses_tpm_firmware_boolean_fields() {
        let record = parse_records(SAMPLE)
            .into_iter()
            .find(|record| record.type_id == 45)
            .unwrap();
        let fields = record.fields();
        assert_eq!(raw_field(&fields, "updatable"), Some("No"));
        assert_eq!(
            parse_bool(raw_field(&fields, "updatable").unwrap()),
            Some(false)
        );
    }
}
