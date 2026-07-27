use super::util::{add_string, basename, list_dirs, parse_hex_u16, read_trimmed, symlink_basename};
use crate::model::{Device, DeviceClass, PropertyValue, SnapshotBuilder};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

pub(super) fn collect(builder: &mut SnapshotBuilder) {
    let root = Path::new("/sys/bus/pci/devices");
    if !root.exists() {
        return;
    }
    let ids = PciIds::load();
    for path in list_dirs(root) {
        let Some(address) = basename(&path) else {
            continue;
        };
        let vendor_id = read_trimmed(path.join("vendor")).and_then(|v| parse_hex_u16(&v));
        let device_id = read_trimmed(path.join("device")).and_then(|v| parse_hex_u16(&v));
        let class_raw = read_trimmed(path.join("class")).unwrap_or_default();
        // A PCI function is transport/controller inventory. Functional
        // collectors (GPU, network, block, USB) create the user-facing device
        // records and may merge back into this ID when it represents the same
        // physical device. Keeping raw PCI functions in the PCI class prevents
        // controllers from appearing as duplicate disks or interfaces.
        let class = DeviceClass::Pci;
        let vendor_name = vendor_id.and_then(|id| ids.vendors.get(&id).cloned());
        let model_name = vendor_id
            .zip(device_id)
            .and_then(|key| ids.devices.get(&key).cloned());
        let name = match (&vendor_name, &model_name) {
            (Some(vendor), Some(model)) => format!("{vendor} {model}"),
            (_, Some(model)) => model.clone(),
            (Some(vendor), None) => format!("{vendor} PCI device"),
            _ => format!("PCI device {address}"),
        };
        let mut device = Device::new(format!("pci:{address}"), class, name);
        device.vendor = vendor_name;
        device.model = model_name;
        device.bus_address = Some(address.clone());
        device.driver = symlink_basename(path.join("driver"));
        add_string(
            &mut device.properties,
            "vendor_id",
            read_trimmed(path.join("vendor")),
        );
        add_string(
            &mut device.properties,
            "device_id",
            read_trimmed(path.join("device")),
        );
        add_string(
            &mut device.properties,
            "subsystem_vendor_id",
            read_trimmed(path.join("subsystem_vendor")),
        );
        add_string(
            &mut device.properties,
            "subsystem_device_id",
            read_trimmed(path.join("subsystem_device")),
        );
        add_string(
            &mut device.properties,
            "class_code",
            (!class_raw.is_empty()).then_some(class_raw),
        );
        add_string(
            &mut device.properties,
            "revision",
            read_trimmed(path.join("revision")),
        );
        add_string(
            &mut device.properties,
            "current_link_speed",
            read_trimmed(path.join("current_link_speed")),
        );
        add_string(
            &mut device.properties,
            "current_link_width",
            read_trimmed(path.join("current_link_width")),
        );
        add_string(
            &mut device.properties,
            "maximum_link_speed",
            read_trimmed(path.join("max_link_speed")),
        );
        add_string(
            &mut device.properties,
            "maximum_link_width",
            read_trimmed(path.join("max_link_width")),
        );
        if let Some(group) = symlink_basename(path.join("iommu_group")) {
            device
                .properties
                .insert("iommu_group".to_owned(), group.into());
        }
        if let Ok(resource) = fs::metadata(path.join("resource")) {
            device.properties.insert(
                "resource_table_bytes".to_owned(),
                PropertyValue::Unsigned(resource.len()),
            );
        }
        builder.add_device(device);
    }
}

#[derive(Default)]
struct PciIds {
    vendors: HashMap<u16, String>,
    devices: HashMap<(u16, u16), String>,
}

impl PciIds {
    fn load() -> Self {
        let candidates = [
            "/usr/share/hwdata/pci.ids",
            "/usr/share/misc/pci.ids",
            "/usr/share/pci.ids",
        ];
        let Some(content) = candidates
            .iter()
            .find_map(|path| fs::read_to_string(path).ok())
        else {
            return Self::default();
        };
        let mut ids = Self::default();
        let mut current_vendor = None;
        for line in content.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with("C ") {
                current_vendor = None;
                continue;
            }
            if !line.starts_with('\t') {
                let Some((id, name)) = parse_pci_id_entry(line) else {
                    continue;
                };
                ids.vendors.insert(id, name.to_owned());
                current_vendor = Some(id);
            } else if !line.starts_with("\t\t") {
                let Some((id, name)) = parse_pci_id_entry(line.trim_start_matches('\t')) else {
                    continue;
                };
                if let Some(vendor) = current_vendor {
                    ids.devices.insert((vendor, id), name.to_owned());
                }
            }
        }
        ids
    }
}

fn parse_pci_id_entry(line: &str) -> Option<(u16, &str)> {
    let mut parts = line.splitn(2, char::is_whitespace);
    let id = u16::from_str_radix(parts.next()?, 16).ok()?;
    let name = parts.next()?.trim();
    (!name.is_empty()).then_some((id, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pci_id_entries() {
        assert_eq!(
            parse_pci_id_entry("10de  NVIDIA Corporation"),
            Some((0x10de, "NVIDIA Corporation")),
        );
        assert_eq!(parse_pci_id_entry("not-an-id"), None);
        assert_eq!(parse_pci_id_entry("10de"), None);
    }
}
