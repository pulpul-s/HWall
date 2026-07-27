use super::util::{
    add_bool, add_string, add_u64, basename, command_exists, list_dirs, read_bool01, read_trimmed,
    read_u64, run_command, symlink_basename,
};
use super::CollectOptions;
use crate::model::{Device, DeviceClass, SnapshotBuilder};
use std::path::{Path, PathBuf};

pub(super) fn collect(builder: &mut SnapshotBuilder, options: &CollectOptions) {
    for (path, name) in interfaces() {
        let mut device = Device::new(format!("net:{name}"), DeviceClass::Network, name.clone());
        device.bus_address = Some(name.clone());
        device.driver = symlink_basename(path.join("device/driver"));
        add_dynamic_properties(&mut device, &path);
        add_u64(&mut device.properties, "mtu", read_u64(path.join("mtu")));
        add_string(
            &mut device.properties,
            "duplex",
            read_trimmed(path.join("duplex")),
        );
        add_string(
            &mut device.properties,
            "interface_type",
            read_trimmed(path.join("type")),
        );
        add_string(
            &mut device.properties,
            "ifindex",
            read_trimmed(path.join("ifindex")),
        );
        if options.include_sensitive {
            add_string(
                &mut device.properties,
                "mac_address",
                read_trimmed(path.join("address")),
            );
        }
        if options.allow_helper_commands {
            enrich_ethtool(&mut device, &name);
        }
        builder.add_device(device);
    }
}

pub(super) fn collect_dynamic(builder: &mut SnapshotBuilder) {
    for (path, name) in interfaces() {
        let mut device = Device::new(format!("net:{name}"), DeviceClass::Network, name.clone());
        add_dynamic_properties(&mut device, &path);
        builder.add_device(device);
    }
}

fn interfaces() -> impl Iterator<Item = (PathBuf, String)> {
    list_dirs("/sys/class/net").into_iter().filter_map(|path| {
        let name = basename(&path)?;
        Some((path, name))
    })
}

fn add_dynamic_properties(device: &mut Device, path: &Path) {
    add_string(
        &mut device.properties,
        "operstate",
        read_trimmed(path.join("operstate")),
    );
    add_u64(
        &mut device.properties,
        "speed_mbps",
        read_u64(path.join("speed")),
    );
    add_bool(
        &mut device.properties,
        "carrier",
        read_bool01(path.join("carrier")),
    );
    add_stats(device, path);
}

fn add_stats(device: &mut Device, path: &Path) {
    let derive_rates = path.join("device").exists();
    for key in [
        "rx_bytes",
        "tx_bytes",
        "rx_packets",
        "tx_packets",
        "rx_errors",
        "tx_errors",
        "rx_dropped",
        "tx_dropped",
    ] {
        let value = read_u64(path.join("statistics").join(key));
        add_u64(&mut device.properties, key, value);
        if derive_rates {
            if let Some(value) = value {
                device.counters.insert(key.to_owned(), value);
            }
        }
    }
}

fn enrich_ethtool(device: &mut Device, interface: &str) {
    if !command_exists("ethtool") {
        return;
    }
    let Ok(output) = run_command("ethtool", ["-i", interface]) else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().replace('-', "_");
        let value = value.trim();
        if !value.is_empty() {
            device
                .properties
                .insert(format!("ethtool_{key}"), value.into());
        }
    }
}
