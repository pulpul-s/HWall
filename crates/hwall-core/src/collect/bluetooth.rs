use super::usb::UsbIds;
use super::util::{add_string, run_command};
use crate::model::{Device, DeviceClass, SnapshotBuilder};
use std::collections::BTreeMap;

pub(super) fn collect(builder: &mut SnapshotBuilder, include_sensitive: bool) {
    let Ok(output) = run_command("bluetoothctl", ["devices", "Connected"]) else {
        return;
    };
    if !output.status.success() {
        return;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut devices = stdout
        .lines()
        .filter_map(parse_connected_device)
        .collect::<Vec<_>>();
    devices.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.address.cmp(&right.address))
    });

    let usb_ids = UsbIds::load();
    let mut ids = BTreeMap::<String, usize>::new();
    for listed in devices {
        let info = device_info(&listed.address);
        let name = info
            .as_ref()
            .and_then(|info| info.alias.as_deref())
            .filter(|name| !name.is_empty())
            .unwrap_or(&listed.name)
            .to_owned();
        let base = device_id_base(&name);
        let count = ids.entry(base.clone()).or_default();
        *count += 1;
        let id = if *count == 1 {
            format!("bluetooth:{base}")
        } else {
            format!("bluetooth:{base}-{}", *count)
        };

        let mut device = Device::new(id, DeviceClass::Bluetooth, name);
        if let Some(info) = info {
            device.vendor = info
                .modalias
                .as_deref()
                .and_then(parse_usb_modalias_vendor)
                .and_then(|vendor_id| usb_ids.vendor_name(vendor_id))
                .map(str::to_owned);
            add_string(
                &mut device.properties,
                "device_type",
                info.icon
                    .as_deref()
                    .and_then(device_type)
                    .map(|value| value.to_owned()),
            );
            add_string(
                &mut device.properties,
                "battery",
                info.battery_percent.map(|value| format!("{value}%")),
            );
            add_string(
                &mut device.properties,
                "signal_strength",
                info.rssi.map(|value| format!("{value} dBm")),
            );
        }
        if include_sensitive {
            add_string(&mut device.properties, "mac_address", Some(listed.address));
        }
        builder.add_device(device);
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ListedDevice {
    address: String,
    name: String,
}

fn parse_connected_device(line: &str) -> Option<ListedDevice> {
    let rest = line.trim().strip_prefix("Device ")?;
    let mut parts = rest.splitn(2, char::is_whitespace);
    let address = parts.next()?.trim();
    if !is_bluetooth_address(address) {
        return None;
    }
    let name = parts.next().unwrap_or_default().trim();
    Some(ListedDevice {
        address: address.to_owned(),
        name: if name.is_empty() {
            "Bluetooth device".to_owned()
        } else {
            name.to_owned()
        },
    })
}

fn is_bluetooth_address(value: &str) -> bool {
    let parts = value.split(':').collect::<Vec<_>>();
    parts.len() == 6
        && parts.iter().all(|part| {
            part.len() == 2 && part.chars().all(|character| character.is_ascii_hexdigit())
        })
}

#[derive(Debug, Default, PartialEq, Eq)]
struct BluetoothInfo {
    alias: Option<String>,
    icon: Option<String>,
    modalias: Option<String>,
    battery_percent: Option<u8>,
    rssi: Option<i16>,
}

fn device_info(address: &str) -> Option<BluetoothInfo> {
    let output = run_command("bluetoothctl", ["info", address]).ok()?;
    output
        .status
        .success()
        .then(|| parse_device_info(&String::from_utf8_lossy(&output.stdout)))
}

fn parse_device_info(output: &str) -> BluetoothInfo {
    let mut info = BluetoothInfo::default();
    for line in output.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix("Alias:") {
            info.alias = non_empty(value);
        } else if let Some(value) = line.strip_prefix("Icon:") {
            info.icon = non_empty(value);
        } else if let Some(value) = line.strip_prefix("Modalias:") {
            info.modalias = non_empty(value);
        } else if let Some(value) = line.strip_prefix("Battery Percentage:") {
            info.battery_percent = parse_battery_percentage(value);
        } else if let Some(value) = line.strip_prefix("RSSI:") {
            info.rssi = parse_rssi(value);
        }
    }
    info
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn parse_battery_percentage(value: &str) -> Option<u8> {
    if let Some((_, raw_percent)) = value.rsplit_once('(')
        && let Ok(percent) = raw_percent
            .trim_end_matches(')')
            .trim_end_matches('%')
            .parse::<u8>()
        && percent <= 100
    {
        return Some(percent);
    }

    let value = value.trim();
    let percent = if let Some(hex) = value.strip_prefix("0x") {
        u8::from_str_radix(hex.split_whitespace().next()?, 16).ok()?
    } else {
        value
            .trim_end_matches('%')
            .split_whitespace()
            .next()?
            .parse()
            .ok()?
    };
    (percent <= 100).then_some(percent)
}

fn parse_usb_modalias_vendor(value: &str) -> Option<u16> {
    let vendor = value.trim().strip_prefix("usb:v")?.get(..4)?;
    u16::from_str_radix(vendor, 16).ok()
}

fn parse_rssi(value: &str) -> Option<i16> {
    if let Some((_, decimal)) = value.rsplit_once('(')
        && let Ok(rssi) = decimal.trim_end_matches(')').trim().parse::<i16>()
    {
        return Some(rssi);
    }

    value.split_whitespace().next()?.parse().ok()
}

fn device_type(icon: &str) -> Option<&'static str> {
    match icon {
        "input-mouse" => Some("Mouse"),
        "input-keyboard" => Some("Keyboard"),
        "input-gaming" => Some("Game controller"),
        "audio-headset" => Some("Headset"),
        "audio-headphones" => Some("Headphones"),
        "audio-speakers" => Some("Speakers"),
        _ => None,
    }
}

fn device_id_base(name: &str) -> String {
    let mut id = String::new();
    let mut separator = false;
    for character in name.trim().chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            id.push(character);
            separator = false;
        } else if !separator && !id.is_empty() {
            id.push('-');
            separator = true;
        }
    }
    let id = id.trim_matches('-');
    if id.is_empty() {
        "device".to_owned()
    } else {
        id.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_connected_devices_with_names_containing_spaces() {
        assert_eq!(
            parse_connected_device("Device AA:BB:CC:DD:EE:FF MX Master 3S"),
            Some(ListedDevice {
                address: "AA:BB:CC:DD:EE:FF".to_owned(),
                name: "MX Master 3S".to_owned(),
            })
        );
        assert_eq!(parse_connected_device("Controller AA:BB:CC:DD:EE:FF"), None);
        assert_eq!(parse_connected_device("Device not-an-address Mouse"), None);
    }

    #[test]
    fn parses_device_type_and_battery() {
        let info = parse_device_info(
            "\tAlias: MX Master 3S\n\tIcon: input-mouse\n\tConnected: yes\n\tModalias: usb:v046DpB037d0003\n\tRSSI: -58\n\tBattery Percentage: 0x55 (85)\n",
        );
        assert_eq!(info.alias.as_deref(), Some("MX Master 3S"));
        assert_eq!(info.icon.as_deref().and_then(device_type), Some("Mouse"));
        assert_eq!(info.modalias.as_deref(), Some("usb:v046DpB037d0003"));
        assert_eq!(info.rssi, Some(-58));
        assert_eq!(info.battery_percent, Some(85));
    }

    #[test]
    fn parses_modalias_vendor_and_rssi_formats() {
        assert_eq!(
            parse_usb_modalias_vendor("usb:v046DpB037d0003"),
            Some(0x046d)
        );
        assert_eq!(
            parse_usb_modalias_vendor("usb:v054Cp0CD3d0452"),
            Some(0x054c)
        );
        assert_eq!(parse_usb_modalias_vendor("pci:v00008086"), None);
        assert_eq!(parse_rssi("-58"), Some(-58));
        assert_eq!(parse_rssi("0xffc6 (-58)"), Some(-58));
        assert_eq!(parse_rssi("unknown"), None);
    }

    #[test]
    fn parses_battery_formats_and_device_ids() {
        assert_eq!(parse_battery_percentage("0x64 (100)"), Some(100));
        assert_eq!(parse_battery_percentage("85%"), Some(85));
        assert_eq!(parse_battery_percentage("101%"), None);
        assert_eq!(device_id_base("MX Master 3S"), "mx-master-3s");
        assert_eq!(device_id_base("  !!!  "), "device");
    }
}
