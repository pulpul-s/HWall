use super::util::{add_string, read_trimmed};
use crate::model::{Device, DeviceClass, SnapshotBuilder};
use std::collections::BTreeMap;
use std::fs;

pub(super) fn collect(builder: &mut SnapshotBuilder) {
    let mut device = Device::new("system:0", DeviceClass::System, "Linux system");
    add_string(
        &mut device.properties,
        "kernel_release",
        read_trimmed("/proc/sys/kernel/osrelease"),
    );
    add_string(
        &mut device.properties,
        "kernel_version",
        read_trimmed("/proc/sys/kernel/version"),
    );
    add_string(
        &mut device.properties,
        "architecture",
        Some(std::env::consts::ARCH.to_owned()),
    );
    add_string(
        &mut device.properties,
        "hostname",
        read_trimmed("/proc/sys/kernel/hostname"),
    );

    for (key, value) in parse_os_release() {
        device.properties.insert(format!("os_{key}"), value.into());
    }
    builder.add_device(device);
}

fn parse_os_release() -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    let content = fs::read_to_string("/etc/os-release")
        .or_else(|_| fs::read_to_string("/usr/lib/os-release"))
        .unwrap_or_default();
    for line in content.lines() {
        let Some((key, raw)) = line.split_once('=') else {
            continue;
        };
        let value = raw.trim().trim_matches('"').replace("\\\"", "\"");
        if !value.is_empty() {
            values.insert(key.to_ascii_lowercase(), value);
        }
    }
    values
}
