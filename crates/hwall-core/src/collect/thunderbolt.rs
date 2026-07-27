use super::util::{add_string, basename, list_dirs, read_trimmed};
use crate::model::{Device, DeviceClass, SnapshotBuilder};

pub(super) fn collect(builder: &mut SnapshotBuilder, include_sensitive: bool) {
    for path in list_dirs("/sys/bus/thunderbolt/devices") {
        let Some(name) = basename(&path) else {
            continue;
        };
        let model = read_trimmed(path.join("device_name"));
        let vendor = read_trimmed(path.join("vendor_name"));
        let display_name = match (&vendor, &model) {
            (Some(vendor), Some(model)) => format!("{vendor} {model}"),
            (_, Some(model)) => model.clone(),
            _ => format!("Thunderbolt device {name}"),
        };

        let mut device = Device::new(
            format!("thunderbolt:{name}"),
            DeviceClass::Thunderbolt,
            display_name,
        );
        device.vendor = vendor;
        device.model = model;
        device.bus_address = Some(name);
        add_string(
            &mut device.properties,
            "generation",
            read_trimmed(path.join("generation")),
        );
        add_string(
            &mut device.properties,
            "authorized",
            read_trimmed(path.join("authorized")),
        );
        add_string(
            &mut device.properties,
            "security_level",
            read_trimmed(path.join("security")),
        );
        if include_sensitive {
            add_string(
                &mut device.properties,
                "unique_id",
                read_trimmed(path.join("unique_id")),
            );
        }
        builder.add_device(device);
    }
}
