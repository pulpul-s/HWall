use crate::{present_sensor, sensor_key, AlertRule, AlertState};
use hwall_core::render::{
    format_property_value, format_sample_age, hardware_property_label,
    is_low_level_hardware_property, sensor_kind_name,
};
use hwall_core::{
    is_storage_health_property, natural_cmp, supports_storage_health, Device, DeviceClass, Sensor,
    Snapshot, SnapshotStatistics, StorageHealth, StorageHealthAvailability,
};
use std::borrow::Cow;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HardwareCategoryKind {
    System,
    Motherboard,
    Processor,
    Memory,
    Graphics,
    Storage,
    Network,
    Pci,
    Usb,
    Thunderbolt,
    Audio,
    Power,
    Thermal,
    Security,
    Controllers,
    Other,
}

impl HardwareCategoryKind {
    pub const ALL: [Self; 16] = [
        Self::System,
        Self::Motherboard,
        Self::Processor,
        Self::Memory,
        Self::Graphics,
        Self::Storage,
        Self::Network,
        Self::Pci,
        Self::Usb,
        Self::Thunderbolt,
        Self::Audio,
        Self::Power,
        Self::Thermal,
        Self::Security,
        Self::Controllers,
        Self::Other,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Motherboard => "Motherboard & firmware",
            Self::Processor => "Processor",
            Self::Memory => "Memory",
            Self::Graphics => "Graphics",
            Self::Storage => "Storage",
            Self::Network => "Network",
            Self::Pci => "PCI",
            Self::Usb => "USB",
            Self::Thunderbolt => "Thunderbolt",
            Self::Audio => "Audio",
            Self::Power => "Power & batteries",
            Self::Thermal => "Thermal",
            Self::Security => "Security",
            Self::Controllers => "Sensor controllers",
            Self::Other => "Other",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardwareInventory {
    pub categories: Vec<HardwareCategory>,
}

impl HardwareInventory {
    pub fn device_count(&self) -> usize {
        self.categories
            .iter()
            .map(|category| category.devices.len())
            .sum()
    }

    pub fn devices(&self) -> impl Iterator<Item = &HardwareDevice> {
        self.categories
            .iter()
            .flat_map(|category| category.devices.iter())
    }

    pub fn device(&self, id: &str) -> Option<&HardwareDevice> {
        self.devices().find(|device| device.id == id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardwareCategory {
    pub kind: HardwareCategoryKind,
    pub label: String,
    pub devices: Vec<HardwareDevice>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardwareDevice {
    pub id: String,
    pub category: HardwareCategoryKind,
    pub name: String,
    pub subtitle: String,
    pub sections: Vec<HardwareSection>,
    pub advanced: Vec<HardwareProperty>,
    pub sensors: Vec<HardwareSensor>,
    pub storage_health_refreshable: bool,
    pub storage_health_permission_required: bool,
    search_text: String,
}

impl HardwareDevice {
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        query.is_empty() || self.search_text.contains(&query)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardwareSection {
    pub title: String,
    pub properties: Vec<HardwareProperty>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardwareProperty {
    pub key: String,
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardwareSensor {
    pub id: String,
    pub group: String,
    pub label: String,
    pub current: String,
    pub minimum: String,
    pub maximum: String,
    pub average: String,
    pub status: String,
    pub current_color: Option<String>,
    pub minimum_color: Option<String>,
    pub maximum_color: Option<String>,
    pub average_color: Option<String>,
    pub status_color: Option<String>,
    pub dimmed: bool,
}

pub fn build_hardware_inventory(
    snapshot: &Snapshot,
    statistics: &SnapshotStatistics,
    sensor_aliases: &BTreeMap<String, String>,
    alert_rules: &BTreeMap<String, AlertRule>,
    alert_states: &BTreeMap<String, AlertState>,
) -> HardwareInventory {
    let mut grouped: BTreeMap<HardwareCategoryKind, Vec<HardwareDevice>> = BTreeMap::new();
    for device in snapshot
        .devices
        .iter()
        .filter(|device| hardware_device_visible(device))
    {
        let Some(display_device) = hardware_view_device(snapshot, device) else {
            continue;
        };
        let projected = project_device(
            display_device.as_ref(),
            statistics,
            sensor_aliases,
            alert_rules,
            alert_states,
        );
        grouped
            .entry(projected.category)
            .or_default()
            .push(projected);
    }

    let mut categories = Vec::new();
    for kind in HardwareCategoryKind::ALL {
        let Some(mut devices) = grouped.remove(&kind) else {
            continue;
        };
        devices.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.id.cmp(&right.id))
        });
        categories.push(HardwareCategory {
            kind,
            label: kind.label().to_owned(),
            devices,
        });
    }
    HardwareInventory { categories }
}

fn hardware_view_device<'a>(snapshot: &'a Snapshot, device: &'a Device) -> Option<Cow<'a, Device>> {
    match device.class {
        DeviceClass::Motherboard => {
            let mut board = device.clone();
            normalize_chassis_type(&mut board);
            if let Some(firmware) = linked_firmware(snapshot, &device.id) {
                merge_firmware_properties(&mut board, firmware);
            }
            Some(Cow::Owned(board))
        }
        DeviceClass::Bios if firmware_has_visible_motherboard(snapshot, device) => None,
        DeviceClass::Bios => {
            let mut firmware = device.clone();
            normalize_firmware_properties(&mut firmware);
            Some(Cow::Owned(firmware))
        }
        _ => Some(Cow::Borrowed(device)),
    }
}

fn linked_firmware<'a>(snapshot: &'a Snapshot, motherboard_id: &str) -> Option<&'a Device> {
    snapshot.devices.iter().find(|candidate| {
        candidate.class == DeviceClass::Bios && candidate.parent.as_deref() == Some(motherboard_id)
    })
}

fn firmware_has_visible_motherboard(snapshot: &Snapshot, firmware: &Device) -> bool {
    let Some(parent_id) = firmware.parent.as_deref() else {
        return false;
    };
    snapshot.devices.iter().any(|candidate| {
        candidate.id == parent_id
            && candidate.class == DeviceClass::Motherboard
            && hardware_device_visible(candidate)
    })
}

fn merge_firmware_properties(board: &mut Device, firmware: &Device) {
    let mut firmware = firmware.clone();
    normalize_firmware_properties(&mut firmware);
    if let Some(vendor) = firmware.vendor.take() {
        firmware
            .properties
            .entry("bios_vendor".to_owned())
            .or_insert(vendor.into());
    }
    for (key, value) in firmware.properties {
        board.properties.entry(key).or_insert(value);
    }
}

fn normalize_firmware_properties(firmware: &mut Device) {
    if let Some(version) = firmware.model.take() {
        firmware
            .properties
            .entry("bios_version".to_owned())
            .or_insert(version.into());
    }
    rename_property(firmware, "release_date", "bios_date");
    rename_property(firmware, "bios_release", "platform_firmware_revision");
}

fn rename_property(device: &mut Device, old_key: &str, new_key: &str) {
    if let Some(value) = device.properties.remove(old_key) {
        device.properties.entry(new_key.to_owned()).or_insert(value);
    }
}

fn normalize_chassis_type(board: &mut Device) {
    if let Some(value) = board.properties.remove("chassis_form_factor") {
        board.properties.insert("chassis_type".to_owned(), value);
        return;
    }

    let Some(code) = board
        .property_str("chassis_type")
        .and_then(|value| value.parse::<u8>().ok())
    else {
        return;
    };
    let Some(name) = chassis_type_name(code) else {
        return;
    };
    board
        .properties
        .insert("chassis_type".to_owned(), name.into());
}

fn chassis_type_name(code: u8) -> Option<&'static str> {
    match code & 0x7f {
        1 => Some("Other"),
        2 => Some("Unknown"),
        3 => Some("Desktop"),
        4 => Some("Low-profile desktop"),
        5 => Some("Pizza box"),
        6 => Some("Mini tower"),
        7 => Some("Tower"),
        8 => Some("Portable"),
        9 => Some("Laptop"),
        10 => Some("Notebook"),
        11 => Some("Hand-held"),
        12 => Some("Docking station"),
        13 => Some("All-in-one"),
        14 => Some("Sub-notebook"),
        15 => Some("Space-saving"),
        16 => Some("Lunch box"),
        17 => Some("Main server chassis"),
        18 => Some("Expansion chassis"),
        19 => Some("Sub-chassis"),
        20 => Some("Bus expansion chassis"),
        21 => Some("Peripheral chassis"),
        22 => Some("RAID chassis"),
        23 => Some("Rack-mount chassis"),
        24 => Some("Sealed-case PC"),
        25 => Some("Multi-system chassis"),
        26 => Some("Compact PCI"),
        27 => Some("Advanced TCA"),
        28 => Some("Blade"),
        29 => Some("Blade enclosure"),
        30 => Some("Tablet"),
        31 => Some("Convertible"),
        32 => Some("Detachable"),
        33 => Some("IoT gateway"),
        34 => Some("Embedded PC"),
        35 => Some("Mini PC"),
        36 => Some("Stick PC"),
        _ => None,
    }
}

fn project_device(
    device: &Device,
    statistics: &SnapshotStatistics,
    sensor_aliases: &BTreeMap<String, String>,
    alert_rules: &BTreeMap<String, AlertRule>,
    alert_states: &BTreeMap<String, AlertState>,
) -> HardwareDevice {
    let category = category_for(device);
    let subtitle = device_subtitle(device);
    let storage_health_refreshable = supports_storage_health(device);
    let has_storage_health = device.storage_health.is_some()
        || device
            .properties
            .keys()
            .any(|key| is_storage_health_property(key));
    let mut sections: BTreeMap<&'static str, Vec<HardwareProperty>> = BTreeMap::new();
    let mut advanced = Vec::new();

    push_identity(
        &mut sections,
        "hwall_class",
        "Class",
        Some(device.class.display_name()),
    );
    push_identity(&mut sections, "vendor", "Vendor", device.vendor.as_deref());
    push_identity(&mut sections, "model", "Model", device.model.as_deref());
    push_identity(&mut sections, "driver", "Driver", device.driver.as_deref());
    push_identity(
        &mut sections,
        "bus_address",
        "Bus address",
        device.bus_address.as_deref(),
    );
    if category == HardwareCategoryKind::Storage
        && (storage_health_refreshable || has_storage_health)
    {
        push_storage_health_summary(&mut sections, device.storage_health.as_ref());
    }

    for (key, value) in &device.properties {
        let Some(value) = format_property_value(key, value) else {
            continue;
        };
        let property = HardwareProperty {
            key: key.clone(),
            label: hardware_property_label(key),
            value,
        };
        if advanced_property(key) {
            advanced.push(property);
        } else {
            sections
                .entry(property_section(key))
                .or_default()
                .push(property);
        }
    }

    let section_order = section_order_for(category);
    let mut projected_sections = Vec::new();
    for &title in section_order {
        let Some(mut properties) = sections.remove(title) else {
            continue;
        };
        sort_properties(&mut properties);
        projected_sections.push(HardwareSection {
            title: title.to_owned(),
            properties,
        });
    }
    for (title, mut properties) in sections {
        sort_properties(&mut properties);
        projected_sections.push(HardwareSection {
            title: title.to_owned(),
            properties,
        });
    }
    sort_properties(&mut advanced);

    let mut sensors = device
        .sensors
        .iter()
        .map(|sensor| {
            project_sensor(
                device,
                sensor,
                statistics,
                sensor_aliases,
                alert_rules,
                alert_states,
            )
        })
        .collect::<Vec<_>>();
    sensors.sort_by(|left, right| {
        left.group
            .cmp(&right.group)
            .then_with(|| natural_cmp(&left.label, &right.label))
            .then_with(|| left.id.cmp(&right.id))
    });

    let mut search_parts = vec![
        category.label().to_owned(),
        device.name.clone(),
        subtitle.clone(),
        device.id.clone(),
    ];
    for section in &projected_sections {
        search_parts.push(section.title.clone());
        for property in &section.properties {
            search_parts.push(property.label.clone());
            search_parts.push(property.value.clone());
        }
    }
    for property in &advanced {
        search_parts.push(property.label.clone());
        search_parts.push(property.value.clone());
        search_parts.push(property.key.clone());
    }
    for sensor in &sensors {
        search_parts.push(sensor.group.clone());
        search_parts.push(sensor.label.clone());
    }

    HardwareDevice {
        id: device.id.clone(),
        category,
        name: device.name.clone(),
        subtitle,
        sections: projected_sections,
        advanced,
        sensors,
        storage_health_refreshable,
        storage_health_permission_required: device.storage_health.as_ref().is_some_and(|health| {
            health.availability == StorageHealthAvailability::PermissionDenied
        }),
        search_text: search_parts.join(" ").to_lowercase(),
    }
}

pub fn storage_health_availability_text(availability: StorageHealthAvailability) -> &'static str {
    if availability == StorageHealthAvailability::PermissionDenied {
        "Permission required. Use Refresh as administrator for a one-time refresh."
    } else {
        availability.display_name()
    }
}

fn project_sensor(
    device: &Device,
    sensor: &Sensor,
    statistics: &SnapshotStatistics,
    sensor_aliases: &BTreeMap<String, String>,
    alert_rules: &BTreeMap<String, AlertRule>,
    alert_states: &BTreeMap<String, AlertState>,
) -> HardwareSensor {
    let key = sensor_key(&device.id, &sensor.id);
    let presentation = present_sensor(
        sensor,
        statistics.get(&device.id, &sensor.id).copied(),
        alert_rules.get(&key),
        alert_states
            .get(&key)
            .copied()
            .unwrap_or(AlertState::Normal),
    );

    HardwareSensor {
        id: sensor.id.clone(),
        group: sensor_kind_name(sensor.kind).to_owned(),
        label: sensor_aliases
            .get(&key)
            .filter(|alias| !alias.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| sensor.label.clone()),
        current: presentation.current,
        minimum: presentation.minimum,
        maximum: presentation.maximum,
        average: presentation.average,
        status: presentation.status,
        current_color: presentation.current_color,
        minimum_color: presentation.minimum_color,
        maximum_color: presentation.maximum_color,
        average_color: presentation.average_color,
        status_color: presentation.status_color,
        dimmed: presentation.dimmed,
    }
}

fn push_storage_health_summary(
    sections: &mut BTreeMap<&'static str, Vec<HardwareProperty>>,
    health: Option<&StorageHealth>,
) {
    let properties = sections.entry("SMART / Health").or_default();
    let Some(health) = health else {
        properties.push(HardwareProperty {
            key: "health_status".to_owned(),
            label: "Health status".to_owned(),
            value: "Not checked".to_owned(),
        });
        return;
    };

    properties.push(HardwareProperty {
        key: "health_status".to_owned(),
        label: "Health status".to_owned(),
        value: health.status.to_string(),
    });
    properties.push(HardwareProperty {
        key: "health_availability".to_owned(),
        label: "Refresh state".to_owned(),
        value: storage_health_availability_text(health.availability).to_owned(),
    });
    if let Some(checked) = health.last_success_unix_ms {
        properties.push(HardwareProperty {
            key: "health_last_checked".to_owned(),
            label: "Last checked".to_owned(),
            value: format_sample_age(checked),
        });
    } else if let Some(attempted) = health.last_attempt_unix_ms {
        properties.push(HardwareProperty {
            key: "health_last_attempt".to_owned(),
            label: "Last attempt".to_owned(),
            value: format_sample_age(attempted),
        });
    }
    if !health.sources.is_empty() {
        properties.push(HardwareProperty {
            key: "health_sources".to_owned(),
            label: "Sources".to_owned(),
            value: health.sources.join(", "),
        });
    }
    if let Some(message) = health
        .message
        .as_deref()
        .filter(|message| !message.trim().is_empty())
    {
        properties.push(HardwareProperty {
            key: "health_message".to_owned(),
            label: "Note".to_owned(),
            value: message.to_owned(),
        });
    }
}

fn push_identity(
    sections: &mut BTreeMap<&'static str, Vec<HardwareProperty>>,
    key: &str,
    label: &str,
    value: Option<&str>,
) {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    sections
        .entry("Identity")
        .or_default()
        .push(HardwareProperty {
            key: key.to_owned(),
            label: label.to_owned(),
            value: value.to_owned(),
        });
}

fn category_for(device: &Device) -> HardwareCategoryKind {
    let searchable = format!(
        "{} {} {} {}",
        device.name,
        device.vendor.as_deref().unwrap_or_default(),
        device.model.as_deref().unwrap_or_default(),
        device.property_str("device_class").unwrap_or_default(),
    )
    .to_lowercase();
    let class_code = device
        .property_str("class_code")
        .unwrap_or_default()
        .trim_start_matches("0x");

    if searchable.contains("audio") || searchable.contains("sound") || class_code.starts_with("04")
    {
        return HardwareCategoryKind::Audio;
    }
    if searchable.contains("tpm")
        || searchable.contains("trusted platform")
        || searchable.contains("security")
        || class_code.starts_with("10")
    {
        return HardwareCategoryKind::Security;
    }
    if class_code.starts_with("0c03") {
        return HardwareCategoryKind::Usb;
    }
    if class_code.starts_with("0c05") {
        return HardwareCategoryKind::Controllers;
    }

    match device.class {
        DeviceClass::System => HardwareCategoryKind::System,
        DeviceClass::Motherboard | DeviceClass::Bios => HardwareCategoryKind::Motherboard,
        DeviceClass::Cpu => HardwareCategoryKind::Processor,
        DeviceClass::Memory => HardwareCategoryKind::Memory,
        DeviceClass::Gpu => HardwareCategoryKind::Graphics,
        DeviceClass::Storage => HardwareCategoryKind::Storage,
        DeviceClass::Network => HardwareCategoryKind::Network,
        DeviceClass::Pci => HardwareCategoryKind::Pci,
        DeviceClass::Usb => HardwareCategoryKind::Usb,
        DeviceClass::Thunderbolt => HardwareCategoryKind::Thunderbolt,
        DeviceClass::PowerSupply | DeviceClass::Battery => HardwareCategoryKind::Power,
        DeviceClass::Thermal => HardwareCategoryKind::Thermal,
        DeviceClass::SensorController if device.parent.as_deref() == Some("motherboard:0") => {
            HardwareCategoryKind::Motherboard
        }
        DeviceClass::SensorController => HardwareCategoryKind::Controllers,
        DeviceClass::Other => HardwareCategoryKind::Other,
    }
}

fn device_subtitle(device: &Device) -> String {
    if device.class == DeviceClass::System {
        if let Some(operating_system) = device
            .property_str("os_pretty_name")
            .or_else(|| device.property_str("os_name"))
        {
            return operating_system.to_owned();
        }
    }

    let mut values = Vec::new();
    if device.class == DeviceClass::Memory && device.property_str("memory_role") == Some("module") {
        if let Some(slot) = device
            .property_str("slot_label")
            .or_else(|| device.property_str("locator"))
        {
            values.push(slot.to_owned());
        }
    }
    for value in [
        device.vendor.as_deref(),
        device.model.as_deref(),
        device.driver.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .filter(|value| !value.is_empty() && !device.name.contains(*value))
    {
        if !values.iter().any(|known: &String| known.as_str() == value) {
            values.push(value.to_owned());
        }
    }
    if let Some(bus_address) = device
        .bus_address
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if !values
            .iter()
            .any(|known: &String| known.as_str() == bus_address)
        {
            values.push(bus_address.to_owned());
        }
    }
    if values.is_empty() {
        device.class.display_name().to_owned()
    } else {
        values.join(" · ")
    }
}

fn property_section(key: &str) -> &'static str {
    if key == "chassis_type" {
        "Identity"
    } else if key.starts_with("os_")
        || key.starts_with("kernel_")
        || matches!(key, "hostname" | "architecture")
    {
        "Operating system"
    } else if key.ends_with("_frequency_hz")
        || key.ends_with("_freq_hz")
        || key.starts_with("scaling_")
        || key.contains("performance")
        || matches!(key, "boost_enabled" | "policies_sampled")
    {
        "Performance"
    } else if key.starts_with("bios_")
        || key.starts_with("board_")
        || key.starts_with("chassis_")
        || key.starts_with("product_")
        || key.starts_with("firmware_")
        || matches!(
            key,
            "release_date"
                | "ec_firmware_release"
                | "agesa_version"
                | "smbios_version"
                | "uefi_supported"
                | "platform_firmware_revision"
        )
    {
        "Firmware & platform"
    } else if key.starts_with("management_device") {
        "Monitoring & management"
    } else if key.starts_with("tpm_") || key == "security_role" {
        "Security"
    } else if matches!(
        key,
        "cores"
            | "threads"
            | "sockets"
            | "logical_cpus"
            | "physical_package_id"
            | "core_id"
            | "cache_hierarchy"
            | "affected_cpus"
            | "related_cpus"
            | "cpu_family"
            | "model_name"
            | "socket_designation"
            | "processor_upgrade"
    ) {
        "Topology"
    } else if key.contains("memory")
        || key.starts_with("dimm_")
        || matches!(
            key,
            "size_mb"
                | "total_bytes"
                | "installed_capacity_bytes"
                | "maximum_capacity_bytes"
                | "memory_slots"
                | "capacity_bytes"
                | "slot_label"
                | "locator"
                | "bank_locator"
                | "form_factor"
                | "module_type"
                | "spd_speed"
                | "rank"
                | "data_width"
                | "total_width"
                | "minimum_voltage"
                | "maximum_voltage"
                | "configured_voltage"
                | "error_correction_type"
                | "ce_count"
                | "ue_count"
                | "correctable_errors"
                | "uncorrectable_errors"
        )
    {
        "Memory"
    } else if key.contains("link_")
        || key.ends_with("_id")
        || matches!(
            key,
            "speed_mbps"
                | "mac_address"
                | "mtu"
                | "transport"
                | "interface_type"
                | "usb_version"
                | "class_code"
                | "iommu_group"
                | "number_of_configurations"
        )
    {
        "Interface"
    } else if is_storage_health_property(key) {
        "SMART / Health"
    } else if key.contains("sector")
        || key.contains("block_size")
        || key.contains("io_size")
        || matches!(
            key,
            "scheduler" | "state" | "namespace" | "namespace_count" | "partition"
        )
    {
        "Storage"
    } else if key.starts_with("charge_")
        || key.starts_with("energy_")
        || key.starts_with("voltage_")
        || key.starts_with("current_")
        || key.starts_with("power_")
        || matches!(key, "capacity" | "system_usage")
    {
        "Power"
    } else if key.starts_with("vulnerability_") || key == "security_level" {
        "Security"
    } else {
        "Properties"
    }
}

fn section_order_for(category: HardwareCategoryKind) -> &'static [&'static str] {
    match category {
        HardwareCategoryKind::System => &[
            "Identity",
            "Operating system",
            "Firmware & platform",
            "Properties",
        ],
        HardwareCategoryKind::Motherboard => &[
            "Identity",
            "Firmware & platform",
            "Monitoring & management",
            "Interface",
            "Properties",
        ],
        HardwareCategoryKind::Processor => &[
            "Identity",
            "Topology",
            "Performance",
            "Security",
            "Properties",
        ],
        HardwareCategoryKind::Memory => &["Identity", "Memory", "Properties"],
        HardwareCategoryKind::Storage => &[
            "Identity",
            "Storage",
            "SMART / Health",
            "Interface",
            "Properties",
        ],
        HardwareCategoryKind::Network => &["Identity", "Interface", "Properties"],
        HardwareCategoryKind::Power => &["Identity", "Power", "Properties"],
        HardwareCategoryKind::Security => &["Identity", "Security", "Properties"],
        _ => &["Identity", "Interface", "Performance", "Properties"],
    }
}

fn advanced_property(key: &str) -> bool {
    is_low_level_hardware_property(key)
        || key.ends_with("_path")
        || key.starts_with("raw_")
        || matches!(
            key,
            "security_role"
                | "i2c_bus"
                | "i2c_address"
                | "computed_by"
                | "aggregation"
                | "attribute"
                | "of_node"
                | "drm_node"
                | "management_device_address"
                | "management_device_address_type"
                | "firmware_core_count"
                | "firmware_core_enabled"
                | "firmware_thread_count"
                | "firmware_thread_enabled"
                | "memory_slots_populated"
                | "memory_slots_total"
        )
}

fn sort_properties(properties: &mut [HardwareProperty]) {
    properties.sort_by(|left, right| {
        property_priority(&left.key)
            .cmp(&property_priority(&right.key))
            .then_with(|| left.label.cmp(&right.label))
    });
}

fn property_priority(key: &str) -> usize {
    match key {
        "hwall_class" => 0,
        "vendor" => 1,
        "model" => 2,
        "driver" => 3,
        "bus_address" => 4,
        "os_pretty_name" => 5,
        "kernel_release" => 6,
        "architecture" => 7,
        "hostname" => 8,
        "installed_capacity_bytes"
        | "maximum_capacity_bytes"
        | "capacity_bytes"
        | "total_bytes"
        | "size_mb" => 9,
        "memory_slots" | "memory_slots_populated" | "memory_slots_total" => 10,
        "cores" => 11,
        "threads" | "logical_cpus" => 12,
        _ => 100,
    }
}

fn hardware_device_visible(device: &Device) -> bool {
    match device.class {
        DeviceClass::Storage => {
            !device.id.starts_with("pci:")
                && !device.property_bool("partition").unwrap_or(false)
                && !device.name.starts_with("zram")
        }
        DeviceClass::Network => {
            let name = device.bus_address.as_deref().unwrap_or(&device.name);
            !matches!(name, "lo" | "docker0")
                && !name.starts_with("veth")
                && !name.starts_with("br-")
                && !name.starts_with("virbr")
                && !name.starts_with("tun")
                && !name.starts_with("tap")
        }
        DeviceClass::Usb => {
            let class = device.property_str("device_class").unwrap_or_default();
            !device.name.starts_with("Linux ")
                && class != "09"
                && !device.name.to_ascii_lowercase().contains(" hub")
        }
        DeviceClass::Pci => {
            let class = device.property_str("class_code").unwrap_or_default();
            let normalized = class.trim_start_matches("0x");
            !normalized.starts_with("06")
                && !normalized.starts_with("03")
                && !normalized.starts_with("02")
                && !normalized.starts_with("010802")
                && !device.name.contains("Dummy")
                && !device.name.contains("Root Complex")
                && !device.name.contains("Data Fabric")
                && !device.name.contains("PCIe Switch")
                && !device.name.contains("GPP Bridge")
                && !device.name.contains("IOMMU")
        }
        DeviceClass::Thermal | DeviceClass::SensorController => !device.sensors.is_empty(),
        DeviceClass::Other => !device.properties.is_empty() || !device.sensors.is_empty(),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwall_core::{Identification, SensorKind, Unit};

    #[test]
    fn hardware_view_uses_natural_sensor_ordering() {
        let mut snapshot = Snapshot::new();
        let mut cpu = Device::new("cpu:0", DeviceClass::Cpu, "CPU");
        for number in [10, 2, 1] {
            cpu.sensors.push(Sensor::new(
                format!("cpu:{number}:utilization"),
                format!("CPU {number} utilization"),
                SensorKind::Utilization,
                Unit::Percent,
                Some(0.0),
                "/proc/stat",
                Identification::Inferred,
            ));
        }
        snapshot.devices.push(cpu);

        let inventory = build_hardware_inventory(
            &snapshot,
            &SnapshotStatistics::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        let cpu = inventory.device("cpu:0").expect("CPU hardware device");

        assert_eq!(
            cpu.sensors
                .iter()
                .map(|sensor| sensor.label.as_str())
                .collect::<Vec<_>>(),
            vec![
                "CPU 1 utilization",
                "CPU 2 utilization",
                "CPU 10 utilization",
            ]
        );
    }

    #[test]
    fn groups_devices_and_formats_properties() {
        let mut snapshot = Snapshot::new();
        let mut cpu = Device::new("cpu:0", DeviceClass::Cpu, "Example CPU");
        cpu.parent = Some("system:0".to_owned());
        cpu.vendor = Some("Example".to_owned());
        cpu.properties.insert("cores".to_owned(), 8_u64.into());
        cpu.properties
            .insert("base_frequency_hz".to_owned(), 3_500_000_000_u64.into());
        cpu.sensors.push(Sensor::new(
            "temperature:package",
            "Package",
            SensorKind::Temperature,
            Unit::Celsius,
            Some(50.0),
            "/sys/example",
            Identification::KernelLabel,
        ));
        snapshot.devices.push(cpu);

        let inventory = build_hardware_inventory(
            &snapshot,
            &SnapshotStatistics::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert_eq!(inventory.device_count(), 1);
        let device = inventory.device("cpu:0").expect("CPU device");
        assert_eq!(device.category, HardwareCategoryKind::Processor);
        assert!(device.matches("3500.0 mhz"));
        let identity = device
            .sections
            .iter()
            .find(|section| section.title == "Identity")
            .expect("Identity section");
        assert!(identity
            .properties
            .iter()
            .any(|property| property.label == "Class" && property.value == "CPU"));
        assert_eq!(device.sensors[0].current, "50.0 °C");
        assert_eq!(device.sensors[0].status, "Normal");
        assert!(device.advanced.is_empty());
    }

    #[test]
    fn uses_meaningful_hardware_subtitles() {
        let mut system = Device::new("system:0", DeviceClass::System, "Linux system");
        system
            .properties
            .insert("os_pretty_name".to_owned(), "CachyOS Linux".into());

        let mut system_without_pretty_name =
            Device::new("system:1", DeviceClass::System, "Linux system");
        system_without_pretty_name
            .properties
            .insert("os_name".to_owned(), "CachyOS".into());

        let mut usb = Device::new("usb:1-11", DeviceClass::Usb, "USB device 1-11");
        usb.bus_address = Some("1-11".to_owned());

        let motherboard = Device::new(
            "motherboard:0",
            DeviceClass::Motherboard,
            "Example motherboard",
        );

        let mut storage = Device::new("block:sda", DeviceClass::Storage, "Example drive");
        storage.vendor = Some("Example vendor".to_owned());

        assert_eq!(device_subtitle(&system), "CachyOS Linux");
        assert_eq!(device_subtitle(&system_without_pretty_name), "CachyOS");
        assert_eq!(device_subtitle(&usb), "1-11");
        assert_eq!(device_subtitle(&motherboard), "Motherboard");
        assert_eq!(device_subtitle(&storage), "Example vendor");
    }

    #[test]
    fn detects_audio_and_security_devices() {
        let audio = Device::new("pci:audio", DeviceClass::Pci, "HD Audio Controller");
        let security = Device::new("other:tpm", DeviceClass::Other, "Trusted Platform Module");
        assert_eq!(category_for(&audio), HardwareCategoryKind::Audio);
        assert_eq!(category_for(&security), HardwareCategoryKind::Security);
    }

    #[test]
    fn hides_virtual_and_duplicate_transport_devices() {
        let loopback = Device::new("net:lo", DeviceClass::Network, "lo");
        let partition = {
            let mut device = Device::new("block:sda1", DeviceClass::Storage, "sda1");
            device
                .properties
                .insert("partition".to_owned(), true.into());
            device
        };
        let bridge = {
            let mut device = Device::new("pci:bridge", DeviceClass::Pci, "PCI bridge");
            device
                .properties
                .insert("class_code".to_owned(), "0604".into());
            device
        };
        assert!(!hardware_device_visible(&loopback));
        assert!(!hardware_device_visible(&partition));
        assert!(!hardware_device_visible(&bridge));
    }

    #[test]
    fn places_dmi_firmware_memory_and_tpm_information() {
        let mut snapshot = Snapshot::new();

        let mut board = Device::new(
            "motherboard:0",
            DeviceClass::Motherboard,
            "Example motherboard",
        );
        board
            .properties
            .insert("agesa_version".to_owned(), "ComboAm5PI 1.3.0.1".into());
        board
            .properties
            .insert("management_device".to_owned(), "Nuvoton NCT6799D-R".into());
        snapshot.devices.push(board);

        let mut memory = Device::new("memory:system", DeviceClass::Memory, "System memory");
        memory
            .properties
            .insert("installed_capacity_bytes".to_owned(), (64_u64 << 30).into());
        memory
            .properties
            .insert("maximum_capacity_bytes".to_owned(), (256_u64 << 30).into());
        memory
            .properties
            .insert("memory_slots".to_owned(), "2 of 4".into());
        snapshot.devices.push(memory);

        let mut tpm = Device::new(
            "security:tpm:0",
            DeviceClass::Other,
            "Trusted Platform Module",
        );
        tpm.properties
            .insert("security_role".to_owned(), "tpm".into());
        tpm.properties
            .insert("tpm_specification".to_owned(), "2.0".into());
        snapshot.devices.push(tpm);

        let inventory = build_hardware_inventory(
            &snapshot,
            &SnapshotStatistics::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );

        let board = inventory.device("motherboard:0").unwrap();
        assert!(board.sections.iter().any(|section| {
            section.title == "Firmware & platform"
                && section
                    .properties
                    .iter()
                    .any(|property| property.label == "AGESA version")
        }));
        assert!(board.sections.iter().any(|section| {
            section.title == "Monitoring & management"
                && section
                    .properties
                    .iter()
                    .any(|property| property.label == "Management controller")
        }));

        let memory = inventory.device("memory:system").unwrap();
        assert!(memory.sections.iter().any(|section| {
            section.title == "Memory"
                && section
                    .properties
                    .iter()
                    .any(|property| property.label == "Slots populated")
        }));

        let tpm = inventory.device("security:tpm:0").unwrap();
        assert_eq!(tpm.category, HardwareCategoryKind::Security);
        assert!(tpm
            .sections
            .iter()
            .any(|section| section.title == "Security"));
    }

    #[test]
    fn merges_linked_system_firmware_into_the_motherboard_hardware_view() {
        let mut snapshot = Snapshot::new();

        let mut board = Device::new(
            "motherboard:0",
            DeviceClass::Motherboard,
            "ASUSTeK COMPUTER INC. ROG STRIX B650E-E GAMING WIFI",
        );
        board.vendor = Some("ASUSTeK COMPUTER INC.".to_owned());
        board.model = Some("ROG STRIX B650E-E GAMING WIFI".to_owned());
        board
            .properties
            .insert("chassis_type".to_owned(), "3".into());
        board
            .properties
            .insert("smbios_version".to_owned(), "3.6.0".into());
        snapshot.devices.push(board);

        let mut firmware = Device::new("bios:0", DeviceClass::Bios, "System firmware");
        firmware.parent = Some("motherboard:0".to_owned());
        firmware.vendor = Some("American Megatrends Inc.".to_owned());
        firmware.model = Some("3854".to_owned());
        firmware
            .properties
            .insert("release_date".to_owned(), "04/03/2026".into());
        firmware
            .properties
            .insert("bios_release".to_owned(), "38.54".into());
        firmware.properties.insert(
            "firmware_boot_status".to_owned(),
            "No errors detected".into(),
        );
        firmware
            .properties
            .insert("uefi_supported".to_owned(), true.into());
        firmware.properties.insert(
            "firmware_vendor_extension".to_owned(),
            "Example capability".into(),
        );
        snapshot.devices.push(firmware);

        let inventory = build_hardware_inventory(
            &snapshot,
            &SnapshotStatistics::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );

        let board = inventory.device("motherboard:0").unwrap();
        let identity = board
            .sections
            .iter()
            .find(|section| section.title == "Identity")
            .unwrap();
        assert!(identity
            .properties
            .iter()
            .any(|property| { property.label == "Chassis type" && property.value == "Desktop" }));

        let platform = board
            .sections
            .iter()
            .find(|section| section.title == "Firmware & platform")
            .unwrap();
        for (label, value) in [
            ("BIOS vendor", "American Megatrends Inc."),
            ("BIOS version", "3854"),
            ("BIOS release date", "04/03/2026"),
            ("Firmware boot status", "No errors detected"),
            ("Firmware vendor extension", "Example capability"),
            ("Platform firmware revision", "38.54"),
            ("SMBIOS version", "3.6.0"),
            ("UEFI support", "Yes"),
        ] {
            assert!(platform
                .properties
                .iter()
                .any(|property| property.label == label && property.value == value));
        }

        assert!(inventory.device("bios:0").is_none());
        assert_eq!(inventory.device_count(), 1);
    }

    #[test]
    fn keeps_unlinked_system_firmware_as_a_fallback_hardware_device() {
        let mut snapshot = Snapshot::new();
        let mut firmware = Device::new("bios:0", DeviceClass::Bios, "System firmware");
        firmware.vendor = Some("American Megatrends Inc.".to_owned());
        firmware.model = Some("3854".to_owned());
        snapshot.devices.push(firmware);

        let inventory = build_hardware_inventory(
            &snapshot,
            &SnapshotStatistics::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );

        let firmware = inventory.device("bios:0").unwrap();
        let identity = firmware
            .sections
            .iter()
            .find(|section| section.title == "Identity")
            .unwrap();
        assert!(!identity
            .properties
            .iter()
            .any(|property| property.label == "Model"));
        assert!(firmware.sections.iter().any(|section| {
            section.title == "Firmware & platform"
                && section
                    .properties
                    .iter()
                    .any(|property| property.label == "BIOS version" && property.value == "3854")
        }));
    }
}
