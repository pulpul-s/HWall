use super::util::{add_string, basename, list_dirs, read_trimmed};
use crate::model::{Device, DeviceClass, SnapshotBuilder};
use std::path::Path;

pub(super) fn collect(builder: &mut SnapshotBuilder, include_sensitive: bool) {
    let root = Path::new("/sys/bus/usb/devices");
    for path in list_dirs(root) {
        if !path.join("idVendor").exists() || !path.join("idProduct").exists() {
            continue;
        }
        let Some(address) = basename(&path) else {
            continue;
        };
        let manufacturer = read_trimmed(path.join("manufacturer"));
        let product = read_trimmed(path.join("product"));
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
        add_string(
            &mut device.properties,
            "vendor_id",
            read_trimmed(path.join("idVendor")),
        );
        add_string(
            &mut device.properties,
            "product_id",
            read_trimmed(path.join("idProduct")),
        );
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
