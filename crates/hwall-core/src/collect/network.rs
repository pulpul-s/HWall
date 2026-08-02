use super::util::{
    add_bool, add_string, add_u64, basename, canonical, command_exists, list_dirs,
    pci_address_from_path, read_bool01, read_trimmed, read_u64, run_command, symlink_basename,
};
use super::CollectOptions;
use crate::model::{Device, DeviceClass, SnapshotBuilder};
use std::path::{Path, PathBuf};

pub(super) fn collect(builder: &mut SnapshotBuilder, options: &CollectOptions) {
    for (path, name) in interfaces() {
        let kind = interface_kind(&path, &name);
        let mut device = Device::new(
            format!("net:{name}"),
            DeviceClass::Network,
            interface_kind_name(kind),
        );
        device.bus_address = Some(name.clone());
        device.driver = symlink_basename(path.join("device/driver"));
        add_string(&mut device.properties, "network_kind", Some(kind.to_owned()));
        add_string(
            &mut device.properties,
            "pci_address",
            direct_pci_address(&path),
        );
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
        let kind = interface_kind(&path, &name);
        let mut device = Device::new(
            format!("net:{name}"),
            DeviceClass::Network,
            interface_kind_name(kind),
        );
        device.bus_address = Some(name);
        add_string(&mut device.properties, "network_kind", Some(kind.to_owned()));
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

fn direct_pci_address(path: &Path) -> Option<String> {
    if symlink_basename(path.join("device/subsystem")).as_deref() != Some("pci") {
        return None;
    }
    canonical(path.join("device")).and_then(pci_address_from_path)
}

fn interface_kind(path: &Path, name: &str) -> &'static str {
    if name == "lo" {
        return "loopback";
    }
    if path.join("wireless").exists() {
        return "wifi";
    }
    if path.join("bridge").exists() {
        return "bridge";
    }
    if path.join("bonding").exists() {
        return "bond";
    }
    if read_trimmed(path.join("uevent"))
        .is_some_and(|value| value.lines().any(|line| line == "DEVTYPE=vlan"))
    {
        return "vlan";
    }

    let lowered = name.to_ascii_lowercase();
    if lowered.starts_with("veth") {
        "veth"
    } else if lowered.starts_with("wg")
        || lowered.contains("-wg")
        || lowered.contains("wireguard")
    {
        "wireguard"
    } else if lowered.starts_with("tap") {
        "tap"
    } else if path.join("tun_flags").exists() || lowered.starts_with("tun") {
        "tun"
    } else if lowered.starts_with("wwan") {
        "mobile"
    } else if path.join("device").exists() {
        match read_trimmed(path.join("type")).as_deref() {
            Some("1") => "ethernet",
            Some("32") => "infiniband",
            _ => "network",
        }
    } else {
        "virtual"
    }
}

fn interface_kind_name(kind: &str) -> &'static str {
    match kind {
        "loopback" => "Loopback",
        "wifi" => "Wi-Fi",
        "bridge" => "Linux bridge",
        "bond" => "Network bond",
        "vlan" => "VLAN interface",
        "veth" => "Virtual Ethernet",
        "tun" => "TUN interface",
        "tap" => "TAP interface",
        "wireguard" => "WireGuard tunnel",
        "mobile" => "Mobile broadband",
        "infiniband" => "InfiniBand",
        "ethernet" => "Ethernet",
        "network" => "Network adapter",
        _ => "Virtual network interface",
    }
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
        if let Some(value) = value {
            device.counters.insert(key.to_owned(), value);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_known_interface_kinds() {
        assert_eq!(interface_kind_name("ethernet"), "Ethernet");
        assert_eq!(interface_kind_name("wifi"), "Wi-Fi");
        assert_eq!(interface_kind_name("wireguard"), "WireGuard tunnel");
        assert_eq!(interface_kind_name("veth"), "Virtual Ethernet");
        assert_eq!(interface_kind_name("unknown"), "Virtual network interface");
    }
}
