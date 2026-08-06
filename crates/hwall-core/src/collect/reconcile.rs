//! Reconciles collector-specific records into physical devices.
//!
//! Linux exposes one NVMe drive through several interfaces: a PCI function, an
//! NVMe controller, one or more block namespaces, and a hwmon device. Collectors
//! intentionally record those interfaces independently. This pass combines the
//! common single-namespace case into one physical storage device while retaining
//! a hierarchy for controllers that expose multiple namespaces.

use super::util::is_nvme_controller_id;
use crate::model::{Device, DeviceClass, PropertyValue, Snapshot};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn apply(snapshot: &mut Snapshot) {
    reconcile_memory_telemetry(snapshot);
    reconcile_network_identity(snapshot);
    coalesce_single_namespace_nvme(snapshot);
    deduplicate_storage_temperatures(snapshot);
    snapshot.sort();
}

fn reconcile_network_identity(snapshot: &mut Snapshot) {
    let pending_addresses = snapshot
        .devices
        .iter()
        .filter(|device| device.class == DeviceClass::Network)
        .filter(|device| {
            device.property_str("pci_address").is_some()
                && device.property_str("vendor_id").is_none()
                && device.property_str("class_code").is_none()
        })
        .filter_map(|device| device.property_str("pci_address").map(str::to_owned))
        .collect::<BTreeSet<_>>();
    if pending_addresses.is_empty() {
        return;
    }

    let pci_devices = snapshot
        .devices
        .iter()
        .filter(|device| device.class == DeviceClass::Pci)
        .filter_map(|device| {
            let address = device.bus_address.as_deref()?;
            pending_addresses
                .contains(address)
                .then(|| (address.to_owned(), NetworkPciIdentity::from(device)))
        })
        .collect::<BTreeMap<_, _>>();

    for device in &mut snapshot.devices {
        if device.class != DeviceClass::Network {
            continue;
        }
        let Some(address) = device.property_str("pci_address").map(str::to_owned) else {
            continue;
        };
        let Some(pci) = pci_devices.get(&address) else {
            continue;
        };
        let kind = device.property_str("network_kind").map(str::to_owned);

        device.vendor = pci.vendor.clone();
        device.model = pci.model.clone();
        if device.driver.is_none() {
            device.driver = pci.driver.clone();
        }
        for (key, value) in &pci.properties {
            device
                .properties
                .entry((*key).to_owned())
                .or_insert_with(|| value.clone());
        }

        if let Some(name) =
            network_hardware_name(pci.vendor.as_deref(), pci.model.as_deref(), kind.as_deref())
        {
            device.name = name;
        }
    }
}

struct NetworkPciIdentity {
    vendor: Option<String>,
    model: Option<String>,
    driver: Option<String>,
    properties: BTreeMap<&'static str, PropertyValue>,
}

impl From<&Device> for NetworkPciIdentity {
    fn from(device: &Device) -> Self {
        const COPIED_PROPERTIES: [&str; 12] = [
            "vendor_id",
            "device_id",
            "subsystem_vendor_id",
            "subsystem_device_id",
            "class_code",
            "revision",
            "current_link_speed",
            "current_link_width",
            "maximum_link_speed",
            "maximum_link_width",
            "iommu_group",
            "resource_table_bytes",
        ];

        Self {
            vendor: device.vendor.clone(),
            model: device.model.clone(),
            driver: device.driver.clone(),
            properties: COPIED_PROPERTIES
                .into_iter()
                .filter_map(|key| {
                    device
                        .properties
                        .get(key)
                        .cloned()
                        .map(|value| (key, value))
                })
                .collect(),
        }
    }
}

fn network_hardware_name(
    vendor: Option<&str>,
    model: Option<&str>,
    kind: Option<&str>,
) -> Option<String> {
    let vendor = vendor.map(concise_vendor_name);
    let model = model.map(str::trim).filter(|value| !value.is_empty());
    match (vendor, model) {
        (Some(vendor), Some(model)) => {
            let model = if kind == Some("ethernet") {
                model
                    .strip_prefix("Ethernet Controller ")
                    .map(|model| format!("{model} Ethernet"))
                    .unwrap_or_else(|| model.to_owned())
            } else {
                model.to_owned()
            };
            if model.starts_with(vendor) {
                Some(model)
            } else {
                Some(format!("{vendor} {model}"))
            }
        }
        (None, Some(model)) => Some(model.to_owned()),
        (Some(vendor), None) => Some(match kind {
            Some("wifi") => format!("{vendor} Wi-Fi"),
            Some("ethernet") => format!("{vendor} Ethernet"),
            _ => format!("{vendor} network adapter"),
        }),
        (None, None) => None,
    }
}

fn concise_vendor_name(value: &str) -> &str {
    let value = value.trim();
    [
        " Semiconductor Co., Ltd.",
        " Semiconductor Corporation",
        " Corporation",
        " Inc.",
    ]
    .into_iter()
    .find_map(|suffix| value.strip_suffix(suffix))
    .unwrap_or(value)
    .trim()
}

fn reconcile_memory_telemetry(snapshot: &mut Snapshot) {
    let mut telemetry = Vec::new();
    let mut retained = Vec::with_capacity(snapshot.devices.len());
    for device in snapshot.devices.drain(..) {
        if is_standalone_memory_telemetry(&device) {
            telemetry.push(device);
        } else {
            retained.push(device);
        }
    }

    let mut available_modules = retained
        .iter()
        .filter(|device| is_dmi_memory_module(device))
        .map(|device| device.id.clone())
        .collect::<Vec<_>>();
    if available_modules.is_empty() || telemetry.is_empty() {
        retained.extend(telemetry);
        snapshot.devices = retained;
        return;
    }

    let mut unmatched = Vec::new();
    for telemetry_device in telemetry {
        let matches = available_modules
            .iter()
            .filter(|module_id| {
                retained
                    .iter()
                    .find(|device| device.id == **module_id)
                    .is_some_and(|module| telemetry_matches_module(&telemetry_device, module))
            })
            .cloned()
            .collect::<Vec<_>>();
        if matches.len() == 1 {
            merge_into_module(
                &mut retained,
                &mut available_modules,
                &matches[0],
                telemetry_device,
                "firmware_locator",
            );
        } else {
            unmatched.push(telemetry_device);
        }
    }

    if unmatched.len() == 1 && available_modules.len() == 1 {
        let module_id = available_modules[0].clone();
        let telemetry_device = unmatched.remove(0);
        merge_into_module(
            &mut retained,
            &mut available_modules,
            &module_id,
            telemetry_device,
            "single_remaining_module",
        );
    } else if can_pair_by_i2c_order(&retained, &available_modules, &unmatched) {
        available_modules.sort_by_key(|id| {
            retained
                .iter()
                .find(|device| device.id == *id)
                .and_then(dmi_record_index)
                .unwrap_or(u64::MAX)
        });
        unmatched.sort_by_key(i2c_sort_key);
        let pairs = available_modules
            .clone()
            .into_iter()
            .zip(std::mem::take(&mut unmatched));
        for (module_id, telemetry_device) in pairs {
            merge_into_module(
                &mut retained,
                &mut available_modules,
                &module_id,
                telemetry_device,
                "inferred_i2c_order",
            );
        }
    }

    retained.extend(unmatched);
    snapshot.devices = retained;
}

fn is_dmi_memory_module(device: &Device) -> bool {
    device.class == DeviceClass::Memory
        && device.property_str("memory_role") == Some("module")
        && dmi_record_index(device).is_some()
}

fn dmi_record_index(device: &Device) -> Option<u64> {
    device
        .properties
        .get("dmi_record_index")
        .and_then(PropertyValue::as_u64)
}

fn is_standalone_memory_telemetry(device: &Device) -> bool {
    device.class == DeviceClass::Memory
        && !is_dmi_memory_module(device)
        && matches!(
            device
                .property_str("hwmon_driver")
                .or(device.driver.as_deref()),
            Some("spd5118" | "jc42")
        )
}

fn telemetry_matches_module(telemetry: &Device, module: &Device) -> bool {
    let Some(locator) = telemetry.property_str("locator") else {
        return false;
    };
    let locator = normalized_locator(locator);
    ["slot_label", "locator", "bank_locator"]
        .into_iter()
        .filter_map(|key| module.property_str(key))
        .any(|candidate| normalized_locator(candidate) == locator)
}

fn normalized_locator(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn can_pair_by_i2c_order(devices: &[Device], module_ids: &[String], telemetry: &[Device]) -> bool {
    if module_ids.len() <= 1 || module_ids.len() != telemetry.len() {
        return false;
    }

    let modules = module_ids
        .iter()
        .filter_map(|id| devices.iter().find(|device| device.id == *id))
        .collect::<Vec<_>>();
    let record_indices = modules
        .iter()
        .filter_map(|device| dmi_record_index(device))
        .collect::<BTreeSet<_>>();
    let slot_labels = modules
        .iter()
        .filter_map(|device| device.property_str("slot_label"))
        .collect::<BTreeSet<_>>();
    if modules.len() != module_ids.len()
        || record_indices.len() != modules.len()
        || slot_labels.len() != modules.len()
    {
        return false;
    }

    let i2c_keys = telemetry
        .iter()
        .filter_map(i2c_sort_key)
        .collect::<BTreeSet<_>>();
    let i2c_buses = i2c_keys
        .iter()
        .map(|(bus, _)| *bus)
        .collect::<BTreeSet<_>>();
    let drivers = telemetry
        .iter()
        .filter_map(|device| {
            device
                .property_str("hwmon_driver")
                .or(device.driver.as_deref())
        })
        .collect::<BTreeSet<_>>();

    i2c_keys.len() == telemetry.len() && i2c_buses.len() == 1 && drivers.len() == 1
}

fn i2c_sort_key(device: &Device) -> Option<(u32, u16)> {
    if let Some(client) = device.bus_address.as_deref() {
        let (bus, address) = client.split_once('-')?;
        return Some((
            bus.parse().ok()?,
            u16::from_str_radix(address.trim_start_matches("0x"), 16).ok()?,
        ));
    }

    let bus = device.property_str("i2c_bus")?.parse().ok()?;
    let address = u16::from_str_radix(
        device.property_str("i2c_address")?.trim_start_matches("0x"),
        16,
    )
    .ok()?;
    Some((bus, address))
}

fn merge_into_module(
    devices: &mut [Device],
    available_modules: &mut Vec<String>,
    module_id: &str,
    telemetry: Device,
    mapping: &str,
) {
    let Some(module) = devices.iter_mut().find(|device| device.id == module_id) else {
        return;
    };
    merge_memory_telemetry(module, telemetry, mapping);
    available_modules.retain(|id| id != module_id);
}

fn merge_memory_telemetry(module: &mut Device, mut telemetry: Device, mapping: &str) {
    if module.driver.is_none() {
        module.driver = telemetry.driver.take();
    }
    if let Some(address) = telemetry.bus_address.take() {
        module
            .properties
            .entry("telemetry_bus_address".to_owned())
            .or_insert(address.into());
    }
    module
        .properties
        .insert("telemetry_mapping".to_owned(), mapping.into());
    for (key, value) in telemetry.properties {
        if !matches!(key.as_str(), "memory_role" | "inventory_source") {
            module.properties.entry(key).or_insert(value);
        }
    }
    for mut sensor in telemetry.sensors {
        sensor
            .metadata
            .insert("module_mapping".to_owned(), mapping.into());
        if let Some(existing) = module
            .sensors
            .iter_mut()
            .find(|existing| existing.id == sensor.id)
        {
            *existing = sensor;
        } else {
            module.sensors.push(sensor);
        }
    }
}

fn coalesce_single_namespace_nvme(snapshot: &mut Snapshot) {
    let mut namespaces_by_controller: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for device in &snapshot.devices {
        if device.class != DeviceClass::Storage
            || device.property_bool("partition").unwrap_or(false)
        {
            continue;
        }
        let Some(parent) = device.parent.as_deref() else {
            continue;
        };
        if is_nvme_controller_id(parent) && is_nvme_namespace(device) {
            namespaces_by_controller
                .entry(parent.to_owned())
                .or_default()
                .push(device.id.clone());
        }
    }

    let merge_ids: BTreeSet<String> = namespaces_by_controller
        .values()
        .filter(|namespaces| namespaces.len() == 1)
        .flatten()
        .cloned()
        .collect();
    if merge_ids.is_empty() {
        annotate_namespace_counts(snapshot, &namespaces_by_controller);
        return;
    }

    let mut namespaces = BTreeMap::new();
    let mut retained = Vec::with_capacity(snapshot.devices.len());
    let mut parent_remap = BTreeMap::new();
    for device in snapshot.devices.drain(..) {
        if merge_ids.contains(&device.id) {
            namespaces.insert(device.id.clone(), device);
        } else {
            retained.push(device);
        }
    }

    for (controller_id, namespace_ids) in &namespaces_by_controller {
        let Some(controller) = retained
            .iter_mut()
            .find(|device| device.id == *controller_id)
        else {
            // The controller collector may be unavailable in restricted
            // environments. Keep the namespace rather than dropping inventory.
            for namespace_id in namespace_ids {
                if let Some(namespace) = namespaces.remove(namespace_id) {
                    retained.push(namespace);
                }
            }
            continue;
        };

        controller.properties.insert(
            "namespace_count".to_owned(),
            PropertyValue::Unsigned(namespace_ids.len() as u64),
        );

        if namespace_ids.len() != 1 {
            continue;
        }
        let Some(mut namespace) = namespaces.remove(&namespace_ids[0]) else {
            continue;
        };
        parent_remap.insert(namespace.id.clone(), controller_id.clone());

        if let Some(controller_name) = controller.bus_address.take() {
            controller
                .properties
                .entry("controller_name".to_owned())
                .or_insert(controller_name.into());
        }
        if let Some(namespace_name) = namespace.bus_address.take() {
            controller
                .properties
                .insert("namespace".to_owned(), namespace_name.clone().into());
            controller.bus_address = Some(namespace_name);
        }

        if controller.model.is_none() {
            controller.model = namespace.model.take();
        }
        if controller.vendor.is_none() {
            controller.vendor = namespace.vendor.take();
        }
        if controller.driver.is_none() {
            controller.driver = namespace.driver.take();
        }
        if is_generic_nvme_name(&controller.name) && !namespace.name.is_empty() {
            controller.name = namespace.name;
        }

        for (key, value) in namespace.properties {
            controller.properties.entry(key).or_insert(value);
        }
        if let Some(health) = namespace.storage_health.take() {
            controller.storage_health = Some(health);
        }
        for sensor in namespace.sensors {
            if let Some(existing) = controller
                .sensors
                .iter_mut()
                .find(|item| item.id == sensor.id)
            {
                *existing = sensor;
            } else {
                controller.sensors.push(sensor);
            }
        }
        controller.counters.append(&mut namespace.counters);
    }

    retained.extend(namespaces.into_values());
    for device in &mut retained {
        let Some(parent) = device.parent.as_ref() else {
            continue;
        };
        if let Some(controller) = parent_remap.get(parent) {
            device.parent = Some(controller.clone());
        }
    }
    snapshot.devices = retained;
}

fn deduplicate_storage_temperatures(snapshot: &mut Snapshot) {
    for device in &mut snapshot.devices {
        if device.class != DeviceClass::Storage {
            continue;
        }
        let has_live_temperature = device.sensors.iter().any(|sensor| {
            sensor.kind == crate::model::SensorKind::Temperature && !sensor.is_intermittent()
        });
        if has_live_temperature {
            device.sensors.retain(|sensor| {
                sensor.kind != crate::model::SensorKind::Temperature || !sensor.is_intermittent()
            });
            continue;
        }

        let mut kept_temperature = None;
        let mut kept_missing_temperature = false;
        device.sensors.retain(|sensor| {
            if sensor.kind != crate::model::SensorKind::Temperature || !sensor.is_intermittent() {
                return true;
            }
            let Some(value) = sensor.value else {
                if kept_missing_temperature {
                    return false;
                }
                kept_missing_temperature = true;
                return true;
            };
            match kept_temperature {
                None => {
                    kept_temperature = Some(value);
                    true
                }
                Some(previous) => (previous - value).abs() > 1.0,
            }
        });
    }
}

fn annotate_namespace_counts(
    snapshot: &mut Snapshot,
    namespaces_by_controller: &BTreeMap<String, Vec<String>>,
) {
    for (controller_id, namespaces) in namespaces_by_controller {
        if let Some(controller) = snapshot
            .devices
            .iter_mut()
            .find(|device| device.id == *controller_id)
        {
            controller.properties.insert(
                "namespace_count".to_owned(),
                PropertyValue::Unsigned(namespaces.len() as u64),
            );
        }
    }
}

fn is_nvme_namespace(device: &Device) -> bool {
    device.bus_address.as_deref().is_some_and(|name| {
        let Some(rest) = name.strip_prefix("nvme") else {
            return false;
        };
        let Some((controller, namespace)) = rest.split_once('n') else {
            return false;
        };
        !controller.is_empty()
            && !namespace.is_empty()
            && controller
                .chars()
                .all(|character| character.is_ascii_digit())
            && namespace
                .chars()
                .all(|character| character.is_ascii_digit())
    })
}

fn is_generic_nvme_name(name: &str) -> bool {
    name == "NVMe" || name.starts_with("NVMe controller ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Identification, Sensor, SensorKind, Unit};

    fn dmi_module(id: &str, index: u64, slot: &str) -> Device {
        let mut device = Device::new(id, DeviceClass::Memory, "Example DIMM");
        device
            .properties
            .insert("memory_role".to_owned(), "module".into());
        device
            .properties
            .insert("dmi_record_index".to_owned(), index.into());
        device
            .properties
            .insert("slot_label".to_owned(), slot.into());
        device
    }

    fn spd_device(id: &str, client: &str, value: f64) -> Device {
        let mut device = Device::new(id, DeviceClass::Memory, "SPD5118 DDR5 memory module");
        device.driver = Some("spd5118".to_owned());
        device.bus_address = Some(client.to_owned());
        device
            .properties
            .insert("hwmon_driver".to_owned(), "spd5118".into());
        device.sensors.push(Sensor::new(
            format!("{id}:temperature"),
            "Module temperature",
            SensorKind::Temperature,
            Unit::Celsius,
            Some(value),
            "/sys/class/hwmon/example",
            Identification::KnownDriverMapping,
        ));
        device
    }

    #[test]
    fn enriches_network_interfaces_from_their_pci_device() {
        let mut pci = Device::new(
            "pci:0000:06:00.0",
            DeviceClass::Pci,
            "Intel Corporation Ethernet Controller I225-V",
        );
        pci.vendor = Some("Intel Corporation".to_owned());
        pci.model = Some("Ethernet Controller I225-V".to_owned());
        pci.bus_address = Some("0000:06:00.0".to_owned());
        pci.driver = Some("igc".to_owned());
        pci.properties.insert("revision".to_owned(), "0x03".into());
        pci.properties
            .insert("class_code".to_owned(), "0x020000".into());
        pci.properties.insert("iommu_group".to_owned(), "17".into());

        let mut network = Device::new("net:eno1", DeviceClass::Network, "Ethernet");
        network.bus_address = Some("eno1".to_owned());
        network
            .properties
            .insert("network_kind".to_owned(), "ethernet".into());
        network
            .properties
            .insert("pci_address".to_owned(), "0000:06:00.0".into());

        let mut snapshot = Snapshot::new();
        snapshot.devices = vec![pci, network];
        apply(&mut snapshot);

        let network = snapshot
            .devices
            .iter()
            .find(|device| device.id == "net:eno1")
            .unwrap();
        assert_eq!(network.name, "Intel I225-V Ethernet");
        assert_eq!(network.vendor.as_deref(), Some("Intel Corporation"));
        assert_eq!(network.model.as_deref(), Some("Ethernet Controller I225-V"));
        assert_eq!(network.driver.as_deref(), Some("igc"));
        assert_eq!(network.property_str("revision"), Some("0x03"));
        assert_eq!(network.property_str("class_code"), Some("0x020000"));
        assert_eq!(network.property_str("iommu_group"), Some("17"));
    }

    #[test]
    fn keeps_factual_wifi_model_names() {
        assert_eq!(
            network_hardware_name(
                Some("Intel Corporation"),
                Some("Wi-Fi 6E(802.11ax) AX210/AX1675* 2x2 [Typhoon Peak]"),
                Some("wifi"),
            )
            .as_deref(),
            Some("Intel Wi-Fi 6E(802.11ax) AX210/AX1675* 2x2 [Typhoon Peak]"),
        );
    }

    #[test]
    fn merges_memory_telemetry_by_firmware_locator() {
        let module = dmi_module("memory:slot:dimm-a2", 0, "DIMM A2");
        let mut telemetry = spd_device("memory:spd5118:0-0050", "0-0050", 44.0);
        telemetry
            .properties
            .insert("locator".to_owned(), "DIMM A2".into());
        let mut snapshot = Snapshot::new();
        snapshot.devices = vec![module, telemetry];

        apply(&mut snapshot);

        assert_eq!(snapshot.devices.len(), 1);
        let module = snapshot.devices.pop().unwrap();
        assert_eq!(module.sensors.len(), 1);
        assert_eq!(
            module.property_str("telemetry_mapping"),
            Some("firmware_locator")
        );
    }

    #[test]
    fn pairs_equal_dimm_and_spd_sets_by_stable_order() {
        let mut snapshot = Snapshot::new();
        snapshot.devices = vec![
            dmi_module("memory:slot:channel-a", 10, "P0 CHANNEL A / DIMM 1"),
            dmi_module("memory:slot:channel-b", 20, "P0 CHANNEL B / DIMM 1"),
            spd_device("memory:spd5118:0-0052", "0-0052", 47.0),
            spd_device("memory:spd5118:0-0050", "0-0050", 43.0),
        ];

        apply(&mut snapshot);

        assert_eq!(snapshot.devices.len(), 2);
        let first = snapshot
            .devices
            .iter()
            .find(|device| device.id == "memory:slot:channel-a")
            .unwrap();
        let second = snapshot
            .devices
            .iter()
            .find(|device| device.id == "memory:slot:channel-b")
            .unwrap();
        assert_eq!(first.sensors[0].value, Some(43.0));
        assert_eq!(second.sensors[0].value, Some(47.0));
        assert_eq!(
            first.property_str("telemetry_mapping"),
            Some("inferred_i2c_order")
        );
    }

    #[test]
    fn keeps_ambiguous_memory_telemetry_standalone() {
        let mut snapshot = Snapshot::new();
        snapshot.devices = vec![
            dmi_module("memory:slot:channel-a", 10, "P0 CHANNEL A / DIMM 1"),
            dmi_module("memory:slot:channel-b", 20, "P0 CHANNEL B / DIMM 1"),
            spd_device("memory:spd5118:0-0050", "0-0050", 43.0),
        ];

        apply(&mut snapshot);

        assert_eq!(snapshot.devices.len(), 3);
        assert!(
            snapshot
                .devices
                .iter()
                .any(|device| device.id == "memory:spd5118:0-0050")
        );
    }

    #[test]
    fn memory_reconciliation_is_idempotent() {
        let mut snapshot = Snapshot::new();
        snapshot.devices = vec![
            dmi_module("memory:slot:dimm-a2", 0, "DIMM A2"),
            spd_device("memory:spd5118:0-0050", "0-0050", 44.0),
        ];

        apply(&mut snapshot);
        let once = snapshot.clone();
        apply(&mut snapshot);
        assert_eq!(snapshot, once);
    }

    #[test]
    fn combines_a_single_namespace_with_its_controller() {
        let mut snapshot = Snapshot::new();

        let mut controller =
            Device::new("block:nvme2", DeviceClass::Storage, "WD_BLACK SN7100 2TB");
        controller.bus_address = Some("nvme2".to_owned());
        controller.driver = Some("nvme".to_owned());
        controller.sensors.push(Sensor::new(
            "block:nvme2:temp1",
            "Composite",
            SensorKind::Temperature,
            Unit::Celsius,
            Some(43.9),
            "/sys/class/hwmon/hwmon2/temp1_input",
            Identification::KernelLabel,
        ));

        let mut namespace =
            Device::new("block:nvme2n1", DeviceClass::Storage, "WD_BLACK SN7100 2TB");
        namespace.parent = Some("block:nvme2".to_owned());
        namespace.bus_address = Some("nvme2n1".to_owned());
        namespace
            .properties
            .insert("capacity_bytes".to_owned(), 2_000_398_934_016_u64.into());
        namespace
            .properties
            .insert("partition".to_owned(), false.into());
        namespace.counters.insert("read_sectors".to_owned(), 42);

        let mut partition = Device::new("block:nvme2n1p1", DeviceClass::Storage, "nvme2n1p1");
        partition.parent = Some("block:nvme2n1".to_owned());
        partition
            .properties
            .insert("partition".to_owned(), true.into());

        snapshot.devices.push(controller);
        snapshot.devices.push(namespace);
        snapshot.devices.push(partition);
        apply(&mut snapshot);

        assert_eq!(snapshot.devices.len(), 2);
        let drive = snapshot
            .devices
            .iter()
            .find(|device| device.id == "block:nvme2")
            .unwrap();
        assert_eq!(drive.id, "block:nvme2");
        assert_eq!(drive.bus_address.as_deref(), Some("nvme2n1"));
        assert_eq!(
            drive.properties.get("controller_name"),
            Some(&PropertyValue::String("nvme2".to_owned()))
        );
        assert!(drive.properties.contains_key("capacity_bytes"));
        assert_eq!(drive.counters.get("read_sectors"), Some(&42));
        assert_eq!(drive.sensors.len(), 1);
        let partition = snapshot
            .devices
            .iter()
            .find(|device| device.id == "block:nvme2n1p1")
            .unwrap();
        assert_eq!(partition.parent.as_deref(), Some("block:nvme2"));
    }

    #[test]
    fn keeps_multiple_namespaces_as_children() {
        let mut snapshot = Snapshot::new();
        snapshot.devices.push(Device::new(
            "block:nvme0",
            DeviceClass::Storage,
            "Example NVMe",
        ));
        for name in ["nvme0n1", "nvme0n2"] {
            let mut namespace = Device::new(format!("block:{name}"), DeviceClass::Storage, name);
            namespace.parent = Some("block:nvme0".to_owned());
            namespace.bus_address = Some(name.to_owned());
            namespace
                .properties
                .insert("partition".to_owned(), false.into());
            snapshot.devices.push(namespace);
        }

        apply(&mut snapshot);

        assert_eq!(snapshot.devices.len(), 3);
        let controller = snapshot
            .devices
            .iter()
            .find(|device| device.id == "block:nvme0")
            .unwrap();
        assert_eq!(
            controller.properties.get("namespace_count"),
            Some(&PropertyValue::Unsigned(2))
        );
    }
}
