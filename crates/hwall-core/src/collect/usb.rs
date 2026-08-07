use super::util::{add_string, basename, list_dirs, parse_hex_u16, read_trimmed};
use crate::model::{Device, DeviceClass, SnapshotBuilder};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

pub(super) fn collect(builder: &mut SnapshotBuilder, include_sensitive: bool) {
    let root = Path::new("/sys/bus/usb/devices");
    let ids = UsbIds::load();
    for path in list_dirs(root) {
        if !path.join("idVendor").exists() || !path.join("idProduct").exists() {
            continue;
        }
        let Some(address) = basename(&path) else {
            continue;
        };
        let vendor_id_raw = read_trimmed(path.join("idVendor"));
        let product_id_raw = read_trimmed(path.join("idProduct"));
        let vendor_id = vendor_id_raw.as_deref().and_then(parse_hex_u16);
        let product_id = product_id_raw.as_deref().and_then(parse_hex_u16);
        let manufacturer = vendor_id
            .and_then(|id| ids.vendors.get(&id).cloned())
            .or_else(|| read_trimmed(path.join("manufacturer")));
        let product = vendor_id
            .zip(product_id)
            .and_then(|key| ids.products.get(&key).cloned())
            .or_else(|| read_trimmed(path.join("product")));
        let name = match (&manufacturer, &product) {
            (Some(vendor), Some(product)) => format!("{vendor} {product}"),
            (_, Some(product)) => product.clone(),
            (Some(vendor), None) => format!("{vendor} USB device"),
            _ => format!("USB device {address}"),
        };
        let mut device = Device::new(format!("usb:{address}"), DeviceClass::Usb, name);
        device.vendor = manufacturer;
        device.model = product;
        device.bus_address = Some(address);
        add_string(&mut device.properties, "vendor_id", vendor_id_raw);
        add_string(&mut device.properties, "product_id", product_id_raw);
        add_string(
            &mut device.properties,
            "usb_version",
            read_trimmed(path.join("version")),
        );
        add_string(
            &mut device.properties,
            "device_version",
            read_trimmed(path.join("bcdDevice")),
        );
        add_string(
            &mut device.properties,
            "speed_mbps",
            read_trimmed(path.join("speed")),
        );
        add_string(
            &mut device.properties,
            "device_class",
            read_trimmed(path.join("bDeviceClass")),
        );
        add_string(
            &mut device.properties,
            "number_of_configurations",
            read_trimmed(path.join("bNumConfigurations")),
        );
        if include_sensitive {
            add_string(
                &mut device.properties,
                "serial",
                read_trimmed(path.join("serial")),
            );
        }
        builder.add_device(device);
    }
}

#[derive(Default)]
struct UsbIds {
    vendors: HashMap<u16, String>,
    products: HashMap<(u16, u16), String>,
}

impl UsbIds {
    fn load() -> Self {
        let candidates = [
            "/usr/share/hwdata/usb.ids",
            "/usr/share/misc/usb.ids",
            "/usr/share/usb.ids",
            "/var/lib/usbutils/usb.ids",
        ];
        let Some(content) = candidates
            .iter()
            .find_map(|path| fs::read_to_string(path).ok())
        else {
            return Self::default();
        };
        Self::parse(&content)
    }

    fn parse(content: &str) -> Self {
        let mut ids = Self::default();
        let mut current_vendor = None;
        for line in content.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if !line.starts_with('\t') {
                current_vendor = None;
                let Some((id, name)) = parse_usb_id_entry(line) else {
                    continue;
                };
                ids.vendors.insert(id, name.to_owned());
                current_vendor = Some(id);
            } else if !line.starts_with("\t\t") {
                let Some((id, name)) = parse_usb_id_entry(line.trim_start_matches('\t')) else {
                    continue;
                };
                if let Some(vendor) = current_vendor {
                    ids.products.insert((vendor, id), name.to_owned());
                }
            }
        }
        ids
    }
}

fn parse_usb_id_entry(line: &str) -> Option<(u16, &str)> {
    let mut parts = line.splitn(2, char::is_whitespace);
    let id = u16::from_str_radix(parts.next()?, 16).ok()?;
    let name = parts.next()?.trim();
    (!name.is_empty()).then_some((id, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_usb_ids() {
        let ids = UsbIds::parse(
            "# Vendors, devices and classes\n8087  Intel Corp.\n\t0032  AX210 Bluetooth\nC 00  (Defined at Interface level)\n",
        );
        assert_eq!(ids.vendors.get(&0x8087).map(String::as_str), Some("Intel Corp."));
        assert_eq!(
            ids.products.get(&(0x8087, 0x0032)).map(String::as_str),
            Some("AX210 Bluetooth")
        );
    }

    #[test]
    fn parses_usb_id_entries() {
        assert_eq!(
            parse_usb_id_entry("8087  Intel Corp."),
            Some((0x8087, "Intel Corp."))
        );
        assert_eq!(parse_usb_id_entry("not-an-id"), None);
        assert_eq!(parse_usb_id_entry("8087"), None);
    }
}
