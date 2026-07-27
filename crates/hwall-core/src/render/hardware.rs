use super::format::{
    device_heading, format_bytes, format_frequency, format_network_speed, format_sample_age,
    hardware_property_label, is_low_level_hardware_property, is_placeholder, numeric_property,
    numeric_value, property, property_to_string, render_sensor_groups, render_sensor_groups_live,
    render_sensor_groups_with, render_sensor_groups_with_live, report_title, section,
    storage_health_property_label, string_property, subsection,
};
use crate::{
    Device, DeviceClass, PropertyValue, Sensor, SensorKind, Snapshot, SnapshotStatistics,
    STORAGE_HEALTH_PROPERTY_KEYS,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

pub(super) fn render(snapshot: &Snapshot, statistics: Option<&SnapshotStatistics>) -> String {
    let mut out = String::new();
    report_title(&mut out, "HWall hardware report");

    render_system(&mut out, snapshot, statistics);
    render_motherboard(&mut out, snapshot, statistics);
    render_cpus(&mut out, snapshot, statistics);
    render_memory(&mut out, snapshot, statistics);
    render_gpus(&mut out, snapshot, statistics);
    render_storage(&mut out, snapshot, statistics);
    render_network(&mut out, snapshot, statistics);
    render_power(&mut out, snapshot, statistics);
    render_usb(&mut out, snapshot, statistics);
    render_thunderbolt(&mut out, snapshot, statistics);
    render_security(&mut out, snapshot, statistics);
    render_controllers(&mut out, snapshot, statistics);
    render_other_sensors(&mut out, snapshot, statistics);
    render_warnings(&mut out, snapshot);

    out
}

fn render_system(out: &mut String, snapshot: &Snapshot, statistics: Option<&SnapshotStatistics>) {
    let systems = devices_of_class(snapshot, DeviceClass::System);
    if systems.is_empty() {
        return;
    }

    section(out, "System");
    for system in systems {
        device_heading(out, &system.name, None);
        if let Some(operating_system) =
            string_property(system, "os_pretty_name").or_else(|| string_property(system, "os_name"))
        {
            property(out, "Operating system", operating_system);
        }
        let preferred = [
            ("os_id", "Distribution ID"),
            ("kernel_release", "Kernel"),
            ("kernel_version", "Kernel build"),
            ("architecture", "Architecture"),
            ("hostname", "Hostname"),
        ];
        render_properties(out, system, &preferred, &["os_pretty_name", "os_name"]);
        if !system.sensors.is_empty() {
            render_device_sensors(out, system, statistics);
        }
        let _ = writeln!(out);
    }
}

fn render_motherboard(
    out: &mut String,
    snapshot: &Snapshot,
    statistics: Option<&SnapshotStatistics>,
) {
    let boards = devices_of_class(snapshot, DeviceClass::Motherboard);
    let controllers: Vec<&Device> = snapshot
        .devices
        .iter()
        .filter(|device| {
            device.class == DeviceClass::SensorController
                && device.parent.as_deref() == Some("motherboard:0")
                && !device.sensors.is_empty()
        })
        .collect();
    let bios = devices_of_class(snapshot, DeviceClass::Bios);

    if boards.is_empty() && controllers.is_empty() && bios.is_empty() {
        return;
    }

    section(out, "Motherboard and firmware");
    for board in boards {
        device_heading(out, &board.name, board.bus_address.as_deref());
        render_identity(out, board);
        let preferred = [
            ("board_version", "Board revision"),
            ("product_name", "System product"),
            ("product_version", "System version"),
            ("chassis_vendor", "Chassis vendor"),
            ("chassis_type", "Chassis type"),
            ("chassis_form_factor", "Chassis form factor"),
            ("smbios_version", "SMBIOS version"),
            ("agesa_version", "AGESA version"),
            ("management_device", "Management controller"),
            ("management_device_type", "Controller type"),
            ("board_serial", "Board serial"),
            ("product_serial", "System serial"),
            ("product_uuid", "System UUID"),
            ("chassis_serial", "Chassis serial"),
        ];
        render_properties(out, board, &preferred, &[]);
        if !board.sensors.is_empty() {
            render_device_sensors(out, board, statistics);
        }
        let _ = writeln!(out);
    }

    for firmware in bios {
        device_heading(out, &firmware.name, None);
        render_identity(out, firmware);
        let preferred = [
            ("release_date", "Release date"),
            ("bios_release", "BIOS release"),
            ("ec_firmware_release", "EC firmware release"),
            ("platform_firmware_revision", "Platform firmware revision"),
            ("uefi_supported", "UEFI support"),
            ("firmware_upgradeable", "Upgradeable"),
            ("firmware_boot_status", "Boot status"),
            ("rom_size", "ROM size"),
        ];
        render_properties(out, firmware, &preferred, &[]);
        let _ = writeln!(out);
    }

    for controller in controllers {
        device_heading(
            out,
            &friendly_sensor_owner(controller),
            controller.bus_address.as_deref(),
        );
        render_identity(out, controller);
        let preferred = [("update_interval_ms", "Update interval")];
        render_properties(out, controller, &preferred, &[]);
        render_device_sensors(out, controller, statistics);
        let _ = writeln!(out);
    }
}

fn render_cpus(out: &mut String, snapshot: &Snapshot, statistics: Option<&SnapshotStatistics>) {
    let cpus = devices_of_class(snapshot, DeviceClass::Cpu);
    if cpus.is_empty() {
        return;
    }

    section(out, "Processor");
    for cpu in cpus {
        device_heading(out, &cpu.name, cpu.bus_address.as_deref());
        render_identity(out, cpu);

        if let Some(topology) = topology_summary(cpu) {
            property(out, "Topology", topology);
        }
        if let Some(drivers) = cpu_sensor_drivers(cpu) {
            property(out, "Sensor providers", drivers);
        }

        let preferred = [
            ("cpu_family", "Family"),
            ("model_number", "Model"),
            ("stepping", "Stepping"),
            ("microcode", "Microcode"),
            ("socket_designation", "Socket"),
            ("processor_upgrade", "Socket type"),
            (
                "firmware_maximum_frequency_hz",
                "Firmware maximum frequency",
            ),
            ("cache_hierarchy", "Cache hierarchy"),
            ("scaling_governor", "Scaling governor"),
            ("energy_performance_preference", "Energy preference"),
            ("minimum_frequency_hz", "Minimum frequency"),
            ("base_frequency_hz", "Base frequency"),
            ("maximum_frequency_hz", "Maximum frequency"),
            ("boost_enabled", "Boost"),
            ("scaling_available_governors", "Available governors"),
        ];
        render_properties(
            out,
            cpu,
            &preferred,
            &["cores", "threads", "sockets", "hwmon_driver", "flags"],
        );
        render_cpu_features(out, cpu);

        render_device_sensors_with(out, cpu, statistics, |sensor| cpu_sensor_label(cpu, sensor));
        render_cpu_temperature_coverage(out, cpu);
        let _ = writeln!(out);
    }
}

fn render_memory(out: &mut String, snapshot: &Snapshot, statistics: Option<&SnapshotStatistics>) {
    let memory = devices_of_class(snapshot, DeviceClass::Memory);
    if memory.is_empty() {
        return;
    }

    section(out, "Memory");
    for device in memory {
        device_heading(
            out,
            &memory_device_name(device),
            device.bus_address.as_deref(),
        );
        render_identity(out, device);
        let preferred = [
            ("installed_capacity_bytes", "Installed capacity"),
            ("maximum_capacity_bytes", "Maximum capacity"),
            ("memory_slots", "Slots populated"),
            ("error_correction_type", "Error correction"),
            ("slot_label", "Slot"),
            ("locator", "Locator"),
            ("bank_locator", "Bank / channel"),
            ("capacity_bytes", "Capacity"),
            ("size_mb", "Capacity"),
            ("memory_type", "Memory type"),
            ("dimm_mem_type", "Memory type"),
            ("form_factor", "Form factor"),
            ("module_type", "Module type"),
            ("memory_technology", "Memory technology"),
            ("memory_operating_mode", "Operating mode"),
            ("spd_speed", "SPD/default speed"),
            ("configured_memory_speed", "Configured speed"),
            ("data_width", "Data width"),
            ("total_width", "Total width"),
            ("rank", "Ranks"),
            ("manufacturer", "Manufacturer"),
            ("part_number", "Part number"),
            ("serial_number", "Serial number"),
            ("i2c_bus", "I²C bus"),
            ("i2c_address", "I²C address"),
            ("edac_mode", "EDAC mode"),
            ("correctable_errors", "Correctable errors"),
            ("uncorrectable_errors", "Uncorrectable errors"),
        ];
        render_properties(
            out,
            device,
            &preferred,
            &[
                "memory_role",
                "sensor_family",
                "inventory_source",
                "dmi_record_index",
                "telemetry_mapping",
                "memory_slots_populated",
                "memory_slots_total",
                "hwmon_driver",
            ],
        );
        render_device_sensors_with(out, device, statistics, |sensor| {
            memory_sensor_label(device, sensor)
        });
        let _ = writeln!(out);
    }
}

fn render_gpus(out: &mut String, snapshot: &Snapshot, statistics: Option<&SnapshotStatistics>) {
    let gpus = devices_of_class(snapshot, DeviceClass::Gpu);
    if gpus.is_empty() {
        return;
    }

    section(out, "Graphics");
    for gpu in gpus {
        device_heading(out, &gpu.name, gpu.bus_address.as_deref());
        render_identity(out, gpu);
        let preferred = [
            ("driver_version", "Driver version"),
            ("drm_driver", "DRM driver"),
            ("drm_node", "DRM node"),
            ("current_link_speed", "Current PCIe speed"),
            ("current_link_width", "Current PCIe width"),
            ("maximum_link_speed", "Maximum PCIe speed"),
            ("maximum_link_width", "Maximum PCIe width"),
            ("power_state", "Power state"),
            ("performance_level", "Performance level"),
            ("gpu_uuid", "GPU UUID"),
        ];
        render_properties(out, gpu, &preferred, &["hwmon_driver"]);
        render_device_sensors(out, gpu, statistics);
        let _ = writeln!(out);
    }
}

fn render_storage(out: &mut String, snapshot: &Snapshot, statistics: Option<&SnapshotStatistics>) {
    let devices: Vec<&Device> = devices_of_class(snapshot, DeviceClass::Storage)
        .into_iter()
        .filter(|device| hardware_device_visible(device))
        .collect();
    if devices.is_empty() {
        return;
    }

    let ids: BTreeSet<&str> = devices.iter().map(|device| device.id.as_str()).collect();
    let mut children: BTreeMap<&str, Vec<&Device>> = BTreeMap::new();
    for device in &devices {
        if let Some(parent) = device.parent.as_deref() {
            if ids.contains(parent) {
                children.entry(parent).or_default().push(*device);
            }
        }
    }

    section(out, "Storage");
    for device in devices.iter().copied().filter(|device| {
        !device
            .parent
            .as_deref()
            .is_some_and(|parent| ids.contains(parent))
    }) {
        render_storage_device(out, device, statistics);
        if let Some(namespaces) = children.get(device.id.as_str()) {
            for namespace in namespaces {
                render_storage_device(out, namespace, statistics);
            }
        }
    }
}

fn render_storage_device(
    out: &mut String,
    device: &Device,
    statistics: Option<&SnapshotStatistics>,
) {
    device_heading(out, &device.name, device.bus_address.as_deref());
    render_identity(out, device);
    if let Some(interface) = storage_interface(device) {
        property(out, "Interface", interface);
    }
    let inventory = [
        ("capacity_bytes", "Capacity"),
        ("firmware_revision", "Firmware"),
        ("transport", "Transport"),
        ("state", "State"),
        ("namespace", "Namespace"),
        ("namespace_count", "Namespaces"),
        ("logical_block_size", "Logical block size"),
        ("physical_block_size", "Physical block size"),
        ("minimum_io_size", "Minimum I/O size"),
        ("optimal_io_size", "Optimal I/O size"),
        ("scheduler", "Scheduler"),
        ("rotational", "Rotational"),
        ("removable", "Removable"),
        ("serial", "Serial number"),
        ("wwid", "WWID"),
    ];
    let mut hidden = vec!["partition", "controller_name", "subsystem"];
    hidden.extend_from_slice(STORAGE_HEALTH_PROPERTY_KEYS);
    render_properties(out, device, &inventory, &hidden);
    render_storage_health(out, device);
    render_device_sensors(out, device, statistics);
    let _ = writeln!(out);
}

fn render_storage_health(out: &mut String, device: &Device) {
    let has_properties = STORAGE_HEALTH_PROPERTY_KEYS
        .iter()
        .any(|key| device.properties.contains_key(*key));
    if device.storage_health.is_none() && !has_properties {
        return;
    }

    subsection(out, "SMART / Health");
    if let Some(health) = device.storage_health.as_ref() {
        property(out, "Health status", health.status.to_string());
        property(out, "Refresh state", health.availability.to_string());
        if let Some(checked) = health.last_success_unix_ms {
            property(out, "Last checked", format_sample_age(checked));
        } else if let Some(attempted) = health.last_attempt_unix_ms {
            property(out, "Last attempt", format_sample_age(attempted));
        }
        if !health.sources.is_empty() {
            property(out, "Sources", health.sources.join(", "));
        }
        if let Some(message) = health.message.as_deref() {
            property(out, "Note", message);
        }
    } else {
        property(out, "Health status", "Not checked");
    }

    for key in STORAGE_HEALTH_PROPERTY_KEYS {
        if let Some(value) = device.properties.get(*key) {
            if let Some(rendered) = format_property_value(key, value) {
                property(out, storage_health_property_label(key), rendered);
            }
        }
    }
}

fn render_network(out: &mut String, snapshot: &Snapshot, statistics: Option<&SnapshotStatistics>) {
    let devices: Vec<&Device> = devices_of_class(snapshot, DeviceClass::Network)
        .into_iter()
        .filter(|device| hardware_device_visible(device))
        .collect();
    if devices.is_empty() {
        return;
    }

    section(out, "Network");
    for interface in devices {
        let heading = network_device_name(snapshot, interface);
        device_heading(out, &heading, interface.bus_address.as_deref());
        render_identity(out, interface);
        let preferred = [
            ("operstate", "State"),
            ("carrier", "Carrier"),
            ("speed_mbps", "Link speed"),
            ("duplex", "Duplex"),
            ("mtu", "MTU"),
            ("mac_address", "MAC address"),
            ("ethtool_driver", "Kernel driver"),
            ("ethtool_version", "Driver version"),
            ("ethtool_firmware_version", "Firmware"),
            ("ethtool_bus_info", "Bus"),
            ("rx_bytes", "Received"),
            ("tx_bytes", "Transmitted"),
            ("rx_packets", "RX packets"),
            ("tx_packets", "TX packets"),
            ("rx_errors", "RX errors"),
            ("tx_errors", "TX errors"),
            ("rx_dropped", "RX dropped"),
            ("tx_dropped", "TX dropped"),
        ];
        render_properties(out, interface, &preferred, &["ifindex", "interface_type"]);
        render_device_sensors(out, interface, statistics);
        let _ = writeln!(out);
    }
}

fn render_power(out: &mut String, snapshot: &Snapshot, statistics: Option<&SnapshotStatistics>) {
    let devices: Vec<&Device> = snapshot
        .devices
        .iter()
        .filter(|device| {
            matches!(
                device.class,
                DeviceClass::PowerSupply | DeviceClass::Battery
            )
        })
        .collect();
    if devices.is_empty() {
        return;
    }

    section(out, "Power and batteries");
    for device in devices {
        device_heading(out, &device.name, device.bus_address.as_deref());
        render_identity(out, device);
        let preferred = [
            ("type", "Type"),
            ("scope", "Scope"),
            ("status", "Status"),
            ("technology", "Technology"),
            ("capacity", "Charge level"),
            ("health", "Health"),
            ("manufacturer", "Manufacturer"),
            ("model_name", "Model"),
            ("serial_number", "Serial number"),
        ];
        render_properties(out, device, &preferred, &[]);
        render_device_sensors(out, device, statistics);
        let _ = writeln!(out);
    }
}

fn render_usb(out: &mut String, snapshot: &Snapshot, statistics: Option<&SnapshotStatistics>) {
    let devices: Vec<&Device> = devices_of_class(snapshot, DeviceClass::Usb)
        .into_iter()
        .filter(|device| hardware_device_visible(device))
        .collect();
    if devices.is_empty() {
        return;
    }

    section(out, "USB devices");
    for device in devices {
        device_heading(out, &device.name, device.bus_address.as_deref());
        render_identity(out, device);
        let preferred = [
            ("speed_mbps", "Link speed"),
            ("usb_version", "USB version"),
            ("device_version", "Device version"),
            ("device_class", "Device class"),
            ("number_of_configurations", "Configurations"),
            ("vendor_id", "Vendor ID"),
            ("product_id", "Product ID"),
            ("serial", "Serial number"),
        ];
        render_properties(out, device, &preferred, &[]);
        render_device_sensors(out, device, statistics);
        let _ = writeln!(out);
    }
}

fn render_thunderbolt(
    out: &mut String,
    snapshot: &Snapshot,
    statistics: Option<&SnapshotStatistics>,
) {
    let devices = devices_of_class(snapshot, DeviceClass::Thunderbolt);
    if devices.is_empty() {
        return;
    }

    section(out, "Thunderbolt and USB4");
    for device in devices {
        device_heading(out, &device.name, device.bus_address.as_deref());
        render_identity(out, device);
        let preferred = [
            ("generation", "Generation"),
            ("authorized", "Authorized"),
            ("security_level", "Security level"),
            ("unique_id", "Unique ID"),
        ];
        render_properties(out, device, &preferred, &[]);
        render_device_sensors(out, device, statistics);
        let _ = writeln!(out);
    }
}

fn render_controllers(
    out: &mut String,
    snapshot: &Snapshot,
    statistics: Option<&SnapshotStatistics>,
) {
    let devices: Vec<&Device> = devices_of_class(snapshot, DeviceClass::Pci)
        .into_iter()
        .filter(|device| hardware_device_visible(device))
        .collect();
    if devices.is_empty() {
        return;
    }

    let groups = [
        ("Storage controllers", ControllerGroup::Storage),
        (
            "Audio and multimedia controllers",
            ControllerGroup::Multimedia,
        ),
        ("USB controllers", ControllerGroup::Usb),
        ("SMBus and I²C controllers", ControllerGroup::Smbus),
        ("Security controllers", ControllerGroup::Security),
        ("System peripherals", ControllerGroup::System),
        ("Other onboard devices", ControllerGroup::Other),
    ];

    section(out, "Controllers and onboard devices");
    for (title, group) in groups {
        let members: Vec<&Device> = devices
            .iter()
            .copied()
            .filter(|device| controller_group(device) == group)
            .collect();
        if members.is_empty() {
            continue;
        }
        subsection(out, title);
        for device in members {
            device_heading(out, &device.name, device.bus_address.as_deref());
            render_identity(out, device);
            let preferred = [
                ("class_code", "PCI class"),
                ("vendor_id", "Vendor ID"),
                ("device_id", "Device ID"),
                ("subsystem_vendor_id", "Subsystem vendor ID"),
                ("subsystem_device_id", "Subsystem device ID"),
                ("revision", "Revision"),
                ("current_link_speed", "Current PCIe speed"),
                ("current_link_width", "Current PCIe width"),
                ("maximum_link_speed", "Maximum PCIe speed"),
                ("maximum_link_width", "Maximum PCIe width"),
                ("iommu_group", "IOMMU group"),
            ];
            render_properties(out, device, &preferred, &[]);
            render_device_sensors(out, device, statistics);
            let _ = writeln!(out);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControllerGroup {
    Storage,
    Multimedia,
    Usb,
    Smbus,
    Security,
    System,
    Other,
}

fn controller_group(device: &Device) -> ControllerGroup {
    let class = string_property(device, "class_code").unwrap_or_default();
    let class = class.trim_start_matches("0x");
    if class.starts_with("01") {
        ControllerGroup::Storage
    } else if class.starts_with("04") {
        ControllerGroup::Multimedia
    } else if class.starts_with("0c03") {
        ControllerGroup::Usb
    } else if class.starts_with("0c05") {
        ControllerGroup::Smbus
    } else if class.starts_with("10") {
        ControllerGroup::Security
    } else if class.starts_with("08") {
        ControllerGroup::System
    } else {
        ControllerGroup::Other
    }
}

fn render_security(out: &mut String, snapshot: &Snapshot, statistics: Option<&SnapshotStatistics>) {
    let devices = snapshot
        .devices
        .iter()
        .filter(|device| device.property_str("security_role") == Some("tpm"))
        .collect::<Vec<_>>();
    if devices.is_empty() {
        return;
    }

    section(out, "Security");
    for device in devices {
        device_heading(out, &device.name, device.bus_address.as_deref());
        render_identity(out, device);
        let preferred = [
            ("tpm_specification", "Specification"),
            ("tpm_firmware_revision", "Firmware revision"),
            ("tpm_firmware_version", "Firmware version"),
            ("tpm_firmware_release_date", "Firmware release date"),
            ("tpm_firmware_state", "Firmware state"),
            ("tpm_firmware_updatable", "Firmware updatable"),
            ("tpm_firmware_write_protected", "Firmware write-protected"),
            ("tpm_characteristics", "Characteristics"),
        ];
        render_properties(out, device, &preferred, &["security_role"]);
        render_device_sensors(out, device, statistics);
        let _ = writeln!(out);
    }
}

fn render_other_sensors(
    out: &mut String,
    snapshot: &Snapshot,
    statistics: Option<&SnapshotStatistics>,
) {
    let devices: Vec<&Device> = snapshot
        .devices
        .iter()
        .filter(|device| {
            matches!(
                device.class,
                DeviceClass::Thermal | DeviceClass::SensorController
            ) && !(device.class == DeviceClass::SensorController
                && device.parent.as_deref() == Some("motherboard:0"))
                && !device.sensors.is_empty()
        })
        .collect();
    if devices.is_empty() {
        return;
    }

    section(out, "Other sensors");
    for device in devices {
        device_heading(out, &device.name, device.bus_address.as_deref());
        render_identity(out, device);
        render_properties(out, device, &[], &["hwmon_driver"]);
        render_device_sensors(out, device, statistics);
        let _ = writeln!(out);
    }
}

fn render_device_sensors(
    out: &mut String,
    device: &Device,
    statistics: Option<&SnapshotStatistics>,
) {
    match statistics {
        Some(statistics) => render_sensor_groups_live(out, device, statistics),
        None => render_sensor_groups(out, device),
    }
}

fn render_device_sensors_with<F>(
    out: &mut String,
    device: &Device,
    statistics: Option<&SnapshotStatistics>,
    label: F,
) where
    F: FnMut(&Sensor) -> String,
{
    match statistics {
        Some(statistics) => render_sensor_groups_with_live(out, device, statistics, label),
        None => render_sensor_groups_with(out, device, label),
    }
}

fn render_warnings(out: &mut String, snapshot: &Snapshot) {
    if snapshot.warnings.is_empty() {
        return;
    }
    section(out, "Warnings");
    for warning in &snapshot.warnings {
        let _ = writeln!(out, "  ! {warning}");
    }
    let _ = writeln!(out);
}

fn render_identity(out: &mut String, device: &Device) {
    if let Some(vendor) = device
        .vendor
        .as_deref()
        .filter(|value| !is_placeholder(value) && !device.name.contains(*value))
    {
        property(out, "Vendor", vendor);
    }
    if let Some(model) = device
        .model
        .as_deref()
        .filter(|value| !is_placeholder(value) && !device.name.contains(*value))
    {
        property(out, "Model", model);
    }
    if let Some(driver) = device
        .driver
        .as_deref()
        .filter(|value| !is_placeholder(value))
    {
        property(out, "Driver", driver);
    }
}

fn render_properties(
    out: &mut String,
    device: &Device,
    preferred: &[(&str, &str)],
    additionally_hidden: &[&str],
) {
    let mut rendered = BTreeSet::new();
    for (key, label) in preferred {
        if let Some(value) = device.properties.get(*key) {
            if let Some(rendered_value) = format_property_value(key, value) {
                property(out, label, rendered_value);
            }
            rendered.insert((*key).to_owned());
        }
    }

    for (key, value) in &device.properties {
        if rendered.contains(key)
            || additionally_hidden.contains(&key.as_str())
            || diagnostic_only_property(key)
        {
            continue;
        }
        if let Some(rendered_value) = format_property_value(key, value) {
            property(out, &hardware_property_label(key), rendered_value);
        }
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

fn diagnostic_only_property(key: &str) -> bool {
    is_low_level_hardware_property(key) || matches!(key, "device_path" | "hwmon_path")
}

pub fn hardware_device_visible(device: &Device) -> bool {
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

fn storage_interface(device: &Device) -> Option<String> {
    if let Some(transport) = string_property(device, "transport") {
        if transport.eq_ignore_ascii_case("pcie") || device.driver.as_deref() == Some("nvme") {
            return Some("NVMe over PCIe".to_owned());
        }
        return Some(transport);
    }
    match device.driver.as_deref() {
        Some("nvme") => Some("NVMe".to_owned()),
        Some("sd") => Some("SATA/SCSI block device".to_owned()),
        Some(driver) => Some(driver.to_owned()),
        None => None,
    }
}

fn network_device_name(snapshot: &Snapshot, interface: &Device) -> String {
    let interface_name = interface.bus_address.as_deref().unwrap_or(&interface.name);
    let pci_address = string_property(interface, "ethtool_bus_info");
    let model = pci_address.as_deref().and_then(|address| {
        snapshot
            .devices
            .iter()
            .find(|device| {
                device.class == DeviceClass::Pci && device.bus_address.as_deref() == Some(address)
            })
            .map(|device| device.name.clone())
    });
    match model {
        Some(model) => format!("{interface_name} — {model}"),
        None if interface.name != interface_name => {
            format!("{interface_name} — {}", interface.name)
        }
        None => interface.name.clone(),
    }
}

fn friendly_sensor_owner(device: &Device) -> String {
    match device.driver.as_deref() {
        Some("asus") => "ASUS motherboard sensor controller".to_owned(),
        Some("nct6775") | Some("nct6779") | Some("nct6798") | Some("nct6799") => {
            format!(
                "{} motherboard sensor controller",
                device.driver.as_deref().unwrap_or("Super I/O")
            )
        }
        _ => device.name.clone(),
    }
}

fn topology_summary(cpu: &Device) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(sockets) = numeric_property(cpu, "sockets") {
        parts.push(format!(
            "{sockets:.0} socket{}",
            if sockets == 1.0 { "" } else { "s" }
        ));
    }
    if let Some(cores) = numeric_property(cpu, "cores") {
        parts.push(format!("{cores:.0} cores"));
    }
    if let Some(threads) = numeric_property(cpu, "threads") {
        parts.push(format!("{threads:.0} threads"));
    }
    (!parts.is_empty()).then(|| parts.join(" • "))
}

fn cpu_sensor_drivers(cpu: &Device) -> Option<String> {
    let mut drivers = BTreeSet::new();
    if let Some(driver) = string_property(cpu, "hwmon_driver") {
        drivers.insert(driver);
    }
    for sensor in &cpu.sensors {
        if let Some(driver) = sensor.metadata_str("driver") {
            drivers.insert(driver.to_owned());
        }
    }
    (!drivers.is_empty()).then(|| drivers.into_iter().collect::<Vec<_>>().join(", "))
}

fn cpu_sensor_label(cpu: &Device, sensor: &Sensor) -> String {
    if sensor.kind != SensorKind::Temperature {
        return sensor.label.clone();
    }
    let driver = sensor
        .metadata
        .get("driver")
        .and_then(|value| match value {
            PropertyValue::String(value) => Some(value.clone()),
            _ => None,
        })
        .or_else(|| string_property(cpu, "hwmon_driver"));

    match (driver.as_deref(), sensor.label.as_str()) {
        (Some("k10temp" | "k8temp" | "zenpower"), "Tctl") => "CPU control (Tctl)".to_owned(),
        (Some("k10temp" | "k8temp" | "zenpower"), "Tdie") => "CPU die (Tdie)".to_owned(),
        (Some("k10temp" | "k8temp" | "zenpower"), label) if label.starts_with("Tccd") => {
            let index = label.trim_start_matches("Tccd");
            format!("CCD {index} ({label})")
        }
        (Some("coretemp"), label) if label.starts_with("Package id ") => {
            format!("CPU package ({label})")
        }
        _ => sensor.label.clone(),
    }
}

fn render_cpu_features(out: &mut String, cpu: &Device) {
    let Some(flags) = string_property(cpu, "flags") else {
        return;
    };
    let flags: Vec<&str> = flags.split_whitespace().collect();
    if flags.is_empty() {
        return;
    }

    subsection(out, "Instruction sets and CPU features");
    for chunk in flags.chunks(8) {
        let _ = writeln!(out, "    {}", chunk.join("  "));
    }
}

fn render_cpu_temperature_coverage(out: &mut String, cpu: &Device) {
    let has_temperature = cpu
        .sensors
        .iter()
        .any(|sensor| sensor.kind == SensorKind::Temperature);
    let has_per_core = cpu.sensors.iter().any(|sensor| {
        sensor.kind == SensorKind::Temperature
            && (sensor.label.starts_with("Core ")
                || sensor
                    .label
                    .to_ascii_lowercase()
                    .contains("core temperature"))
    });
    if !has_temperature || has_per_core {
        return;
    }

    let drivers = cpu_sensor_drivers(cpu).unwrap_or_default();
    if ["k10temp", "k8temp", "zenpower"]
        .iter()
        .any(|driver| drivers.contains(driver))
    {
        property(
            out,
            "Per-core temperatures",
            "Not exposed by the active AMD temperature driver",
        );
    }
}

fn memory_device_name(device: &Device) -> String {
    let location = string_property(device, "slot_label")
        .or_else(|| string_property(device, "locator"))
        .filter(|value| !is_placeholder(value));
    if let Some(location) = location {
        if device.name.contains(&location) {
            device.name.clone()
        } else {
            format!("{location} — {}", device.name)
        }
    } else {
        device.name.clone()
    }
}

fn memory_sensor_label(device: &Device, sensor: &Sensor) -> String {
    if sensor.kind != SensorKind::Temperature {
        return sensor.label.clone();
    }
    let generic = matches!(
        sensor.label.as_str(),
        "Temperature 1" | "Module temperature" | "DIMM temperature"
    );
    if !generic {
        return sensor.label.clone();
    }
    if let Some(location) = string_property(device, "slot_label")
        .or_else(|| string_property(device, "locator"))
        .filter(|value| !is_placeholder(value))
    {
        return format!("{location} temperature");
    }
    if let Some(address) = string_property(device, "i2c_address") {
        let memory_type = string_property(device, "memory_type")
            .or_else(|| string_property(device, "dimm_mem_type"))
            .unwrap_or_else(|| "Memory".to_owned());
        return format!("{memory_type} module at {address} temperature");
    }
    sensor.label.clone()
}

fn devices_of_class(snapshot: &Snapshot, class: DeviceClass) -> Vec<&Device> {
    snapshot
        .devices
        .iter()
        .filter(|device| device.class == class)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Identification, Sensor, Unit};

    #[test]
    fn default_report_integrates_cpu_identity_and_sensors() {
        let mut snapshot = Snapshot::new();
        let mut cpu = Device::new("cpu:0", DeviceClass::Cpu, "Example CPU 9000");
        cpu.driver = Some("example-pstate".to_owned());
        cpu.properties.insert("cores".to_owned(), 8_u64.into());
        cpu.properties.insert("threads".to_owned(), 16_u64.into());
        cpu.properties.insert("sockets".to_owned(), 1_u64.into());
        cpu.sensors.push(Sensor::new(
            "cpu:0:temp",
            "Package",
            SensorKind::Temperature,
            crate::Unit::Celsius,
            Some(42.0),
            "/sys/class/hwmon/hwmon0/temp1_input",
            Identification::KernelLabel,
        ));
        snapshot.devices.push(cpu);

        let report = render(&snapshot, None);
        assert!(report.contains("Example CPU 9000"));
        assert!(report.contains("1 socket • 8 cores • 16 threads"));
        assert!(report.contains("42.0 °C"));
        assert!(!report.contains("Sensors and live telemetry"));
    }

    #[test]
    fn storage_device_contains_capacity_and_temperature_once() {
        let mut snapshot = Snapshot::new();
        let mut drive = Device::new("block:nvme0", DeviceClass::Storage, "Example NVMe");
        drive.bus_address = Some("nvme0n1".to_owned());
        drive.driver = Some("nvme".to_owned());
        drive
            .properties
            .insert("capacity_bytes".to_owned(), 1_000_000_000_000_u64.into());
        drive.sensors.push(Sensor::new(
            "block:nvme0:temp",
            "Composite",
            SensorKind::Temperature,
            Unit::Celsius,
            Some(39.0),
            "/sys/class/hwmon/hwmon0/temp1_input",
            Identification::KernelLabel,
        ));
        snapshot.devices.push(drive);

        let report = render(&snapshot, None);
        assert_eq!(report.matches("Example NVMe [nvme0n1]").count(), 1);
        assert!(report.contains("Capacity"));
        assert!(report.contains("Composite"));
    }
}
