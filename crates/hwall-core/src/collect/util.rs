use crate::model::PropertyValue;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

pub(super) fn read_trimmed(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

pub(super) fn read_u64(path: impl AsRef<Path>) -> Option<u64> {
    read_trimmed(path)?.parse().ok()
}

pub(super) fn read_f64(path: impl AsRef<Path>) -> Option<f64> {
    read_trimmed(path)?.parse().ok()
}

pub(super) fn read_bool01(path: impl AsRef<Path>) -> Option<bool> {
    match read_trimmed(path)?.as_str() {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}

pub(super) fn list_dirs(path: impl AsRef<Path>) -> Vec<PathBuf> {
    sorted_entries(path)
        .into_iter()
        .filter(|path| path.is_dir() || path.is_symlink())
        .collect()
}

pub(super) fn list_entries(path: impl AsRef<Path>) -> Vec<PathBuf> {
    sorted_entries(path)
}

fn sorted_entries(path: impl AsRef<Path>) -> Vec<PathBuf> {
    let mut entries = fs::read_dir(path)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(Result::ok))
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    entries.sort();
    entries
}

pub(super) fn symlink_basename(path: impl AsRef<Path>) -> Option<String> {
    let target = fs::read_link(path).ok()?;
    target.file_name()?.to_str().map(ToOwned::to_owned)
}

pub(super) fn canonical(path: impl AsRef<Path>) -> Option<PathBuf> {
    fs::canonicalize(path).ok()
}

pub(super) fn basename(path: impl AsRef<Path>) -> Option<String> {
    path.as_ref().file_name()?.to_str().map(ToOwned::to_owned)
}

fn strip_hex_prefix(value: &str) -> &str {
    value.strip_prefix("0x").unwrap_or(value)
}

pub(super) fn parse_hex_u16(value: &str) -> Option<u16> {
    u16::from_str_radix(strip_hex_prefix(value.trim()), 16).ok()
}

pub(super) fn command_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(name))
            .find(|candidate| candidate.is_file())
    })
}

pub(super) fn command_exists(name: &str) -> bool {
    command_path(name).is_some()
}

pub(super) fn run_command<I, S>(program: &str, args: I) -> io::Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new(program);
    command.args(args);
    run_output(&mut command)
}

pub(super) fn run_command_elevated<I, S>(program: &str, args: I) -> io::Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let Some(program) = command_path(program) else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "helper command was not found",
        ));
    };
    let mut command = Command::new("pkexec");
    command.arg(program).args(args);
    run_output(&mut command)
}

fn run_output(command: &mut Command) -> io::Result<Output> {
    command
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .output()
}

pub(super) fn add_string(
    properties: &mut BTreeMap<String, PropertyValue>,
    key: &str,
    value: Option<String>,
) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        properties.insert(key.to_owned(), value.into());
    }
}

pub(super) fn add_u64(
    properties: &mut BTreeMap<String, PropertyValue>,
    key: &str,
    value: Option<u64>,
) {
    add_property(properties, key, value);
}

pub(super) fn add_bool(
    properties: &mut BTreeMap<String, PropertyValue>,
    key: &str,
    value: Option<bool>,
) {
    add_property(properties, key, value);
}

fn add_property<T: Into<PropertyValue>>(
    properties: &mut BTreeMap<String, PropertyValue>,
    key: &str,
    value: Option<T>,
) {
    if let Some(value) = value {
        properties.insert(key.to_owned(), value.into());
    }
}

pub(super) fn pci_address_from_path(path: impl AsRef<Path>) -> Option<String> {
    last_path_component(path, is_pci_address).map(|address| normalize_pci_address(&address))
}

pub(super) fn normalize_pci_address(address: &str) -> String {
    let address = address.trim().to_ascii_lowercase();
    if address.len() >= 12 {
        let tail = &address[address.len() - 12..];
        if is_pci_address(tail) {
            return tail.to_owned();
        }
    }
    address
}

fn is_pci_address(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 12 || bytes[4] != b':' || bytes[7] != b':' || bytes[10] != b'.' {
        return false;
    }
    bytes
        .iter()
        .enumerate()
        .all(|(index, byte)| matches!(index, 4 | 7 | 10) || byte.is_ascii_hexdigit())
}

pub(super) fn block_device_from_path(path: impl AsRef<Path>) -> Option<String> {
    last_path_component(path, is_block_device_name)
}

pub(super) fn stable_path_token(path: impl AsRef<Path>) -> String {
    let raw = path
        .as_ref()
        .strip_prefix("/sys/devices")
        .unwrap_or_else(|_| path.as_ref())
        .to_string_lossy();
    let mut token = String::new();
    let mut previous_was_separator = false;

    for character in raw.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            token.push(character);
            previous_was_separator = false;
        } else if !previous_was_separator && !token.is_empty() {
            token.push('-');
            previous_was_separator = true;
        }
    }

    let token = token.trim_matches('-');
    if token.is_empty() {
        "root".to_owned()
    } else {
        token.to_owned()
    }
}

/// Return the Linux I²C client token (for example `6-0053`) embedded in a
/// sysfs path. The left side is the bus number and the right side is the
/// four-digit hexadecimal client address used by the kernel.
pub(super) fn i2c_client_from_path(path: impl AsRef<Path>) -> Option<String> {
    last_path_component(path, is_i2c_client).map(|value| value.to_ascii_lowercase())
}

fn last_path_component(path: impl AsRef<Path>, predicate: impl Fn(&str) -> bool) -> Option<String> {
    path.as_ref()
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .rfind(|component| predicate(component))
        .map(str::to_owned)
}

fn is_i2c_client(value: &str) -> bool {
    let Some((bus, address)) = value.split_once('-') else {
        return false;
    };
    !bus.is_empty()
        && bus.chars().all(|character| character.is_ascii_digit())
        && address.len() == 4
        && address
            .chars()
            .all(|character| character.is_ascii_hexdigit())
}

fn is_block_device_name(value: &str) -> bool {
    let starts_with_letters_and_has_suffix = |prefix: &str| {
        value.strip_prefix(prefix).is_some_and(|suffix| {
            !suffix.is_empty()
                && suffix
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-')
        })
    };

    starts_with_letters_and_has_suffix("sd")
        || starts_with_letters_and_has_suffix("hd")
        || starts_with_letters_and_has_suffix("vd")
        || starts_with_letters_and_has_suffix("xvd")
        || starts_with_letters_and_has_suffix("mmcblk")
        || starts_with_letters_and_has_suffix("nvme")
        || starts_with_letters_and_has_suffix("md")
        || starts_with_letters_and_has_suffix("dm-")
}

pub(super) fn is_virtual_block_device_name(name: &str) -> bool {
    ["loop", "ram", "zram", "fd"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

pub(super) fn is_nvme_controller_name(name: &str) -> bool {
    name.strip_prefix("nvme").is_some_and(|suffix| {
        !suffix.is_empty() && suffix.chars().all(|character| character.is_ascii_digit())
    })
}

pub(super) fn is_nvme_controller_id(id: &str) -> bool {
    id.strip_prefix("block:")
        .is_some_and(is_nvme_controller_name)
}

pub(super) fn humanize_token(value: &str) -> String {
    value
        .trim_matches('_')
        .split(['_', '-'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_leaf_pci_address() {
        let path = Path::new("/sys/devices/pci0000:00/0000:00:01.0/0000:03:00.0/drm/card0");
        assert_eq!(pci_address_from_path(path).as_deref(), Some("0000:03:00.0"));
    }

    #[test]
    fn extracts_i2c_client() {
        let path = Path::new("/sys/devices/pci0000:00/0000:00:14.0/i2c-6/6-0053");
        assert_eq!(i2c_client_from_path(path).as_deref(), Some("6-0053"));
    }

    #[test]
    fn extracts_block_devices() {
        let path = Path::new("/sys/devices/pci0000:00/host0/target0:0:0/0:0:0:0/block/sda");
        assert_eq!(block_device_from_path(path).as_deref(), Some("sda"));

        let path = Path::new("/sys/devices/pci0000:00/0000:02:00.0/nvme/nvme0/nvme0n1");
        assert_eq!(block_device_from_path(path).as_deref(), Some("nvme0n1"));
    }

    #[test]
    fn creates_readable_stable_path_tokens() {
        let path = Path::new("/sys/devices/platform/peci-0/0-30/peci_dimmtemp.0");
        assert_eq!(
            stable_path_token(path),
            "platform-peci-0-0-30-peci-dimmtemp-0"
        );
    }
}
