use super::util::{add_string, basename, list_dirs, read_trimmed};
use crate::model::{Device, DeviceClass, SnapshotBuilder};

pub(super) fn collect(builder: &mut SnapshotBuilder, include_sensitive: bool) {
    for path in list_dirs("/sys/class/nvme") {
        let Some(name) = basename(&path) else {
            continue;
        };
        let model = read_trimmed(path.join("model"));
        let display_name = model
            .clone()
            .unwrap_or_else(|| format!("NVMe controller {name}"));
        let mut device = Device::new(format!("block:{name}"), DeviceClass::Storage, display_name);
        device.model = model;
        device.bus_address = Some(name);
        device.driver = Some("nvme".to_owned());
        add_string(
            &mut device.properties,
            "firmware_revision",
            read_trimmed(path.join("firmware_rev")),
        );
        add_string(
            &mut device.properties,
            "transport",
            read_trimmed(path.join("transport")),
        );
        add_string(
            &mut device.properties,
            "state",
            read_trimmed(path.join("state")),
        );
        add_string(
            &mut device.properties,
            "subsystem",
            read_trimmed(path.join("subsysnqn")),
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
