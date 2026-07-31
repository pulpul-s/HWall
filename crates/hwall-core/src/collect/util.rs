use crate::model::PropertyValue;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

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

pub(super) const HELPER_TIMEOUT: Duration = Duration::from_secs(10);
pub(super) const STORAGE_HELPER_TIMEOUT: Duration = Duration::from_secs(60);

const COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(10);
const MAX_HELPER_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const SYSTEM_COMMAND_DIRS: [&str; 6] = [
    "/usr/bin",
    "/usr/sbin",
    "/usr/local/bin",
    "/usr/local/sbin",
    "/bin",
    "/sbin",
];

pub(super) fn command_path(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        let candidate = PathBuf::from(name);
        return candidate.is_file().then_some(candidate);
    }

    let mut directories = std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .unwrap_or_default();
    directories.extend(SYSTEM_COMMAND_DIRS.into_iter().map(PathBuf::from));
    find_command(name, directories)
}

pub(super) fn system_command_path(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        return None;
    }
    find_command(name, SYSTEM_COMMAND_DIRS.into_iter().map(PathBuf::from))
}

fn find_command(name: &str, directories: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    directories
        .into_iter()
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

pub(super) fn command_exists(name: &str) -> bool {
    command_path(name).is_some()
}

pub(super) fn system_command_exists(name: &str) -> bool {
    system_command_path(name).is_some()
}

pub(super) fn run_command<I, S>(program: &str, args: I) -> io::Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_named_command(program, args, HELPER_TIMEOUT)
}

pub(super) fn run_storage_command<I, S>(program: &str, args: I) -> io::Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_named_command(program, args, STORAGE_HELPER_TIMEOUT)
}

fn run_named_command<I, S>(program: &str, args: I, timeout: Duration) -> io::Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let program = command_path(program).unwrap_or_else(|| PathBuf::from(program));
    let mut command = Command::new(program);
    command.args(args);
    run_command_configured(&mut command, timeout)
}

pub(super) fn run_command_elevated<I, S>(program: &str, args: I) -> io::Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let program = required_system_command(program)?;
    let pkexec = required_system_command("pkexec")?;
    let mut command = Command::new(pkexec);
    command.arg(program).args(args);
    run_output_without_timeout(&mut command)
}

fn required_system_command(name: &str) -> io::Result<PathBuf> {
    system_command_path(name).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("{name} was not found in a system command directory"),
        )
    })
}

pub(super) fn run_command_configured(
    command: &mut Command,
    timeout: Duration,
) -> io::Result<Output> {
    configure_output(command);
    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("helper stdout pipe was unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("helper stderr pipe was unavailable"))?;
    let mut stdout_reader = Some(spawn_output_reader(stdout));
    let mut stderr_reader = Some(spawn_output_reader(stderr));
    let started = Instant::now();

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = take_output_reader(&mut stdout_reader);
                let _ = take_output_reader(&mut stderr_reader);
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("helper command exceeded {timeout:?}"),
                ));
            }
            Ok(None) => {
                let remaining = timeout.saturating_sub(started.elapsed());
                thread::sleep(remaining.min(COMMAND_POLL_INTERVAL));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = take_output_reader(&mut stdout_reader);
                let _ = take_output_reader(&mut stderr_reader);
                return Err(error);
            }
        }
    };

    Ok(Output {
        status,
        stdout: take_output_reader(&mut stdout_reader)?,
        stderr: take_output_reader(&mut stderr_reader)?,
    })
}

fn run_output_without_timeout(command: &mut Command) -> io::Result<Output> {
    configure_output(command);
    command.output()
}

fn configure_output(command: &mut Command) {
    command
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .stdout(Stdio::piped());
}

fn spawn_output_reader<R>(mut reader: R) -> thread::JoinHandle<io::Result<Vec<u8>>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut output = Vec::new();
        let mut buffer = [0_u8; 8192];
        let mut exceeded_limit = false;
        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            let remaining = MAX_HELPER_OUTPUT_BYTES.saturating_sub(output.len());
            output.extend_from_slice(&buffer[..read.min(remaining)]);
            exceeded_limit |= read > remaining;
        }
        if exceeded_limit {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "helper output exceeded 16 MiB",
            ))
        } else {
            Ok(output)
        }
    })
}

fn take_output_reader(
    reader: &mut Option<thread::JoinHandle<io::Result<Vec<u8>>>>,
) -> io::Result<Vec<u8>> {
    let reader = reader
        .take()
        .ok_or_else(|| io::Error::other("helper output reader was already consumed"))?;
    reader
        .join()
        .map_err(|_| io::Error::other("helper output reader panicked"))?
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

    #[test]
    fn captures_helper_output() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf stdout; printf stderr >&2"]);
        let output = run_command_configured(&mut command, Duration::from_secs(1)).unwrap();

        assert!(output.status.success());
        assert_eq!(output.stdout, b"stdout");
        assert_eq!(output.stderr, b"stderr");
    }

    #[test]
    fn times_out_and_reaps_slow_helpers() {
        let mut command = Command::new("/bin/sleep");
        command.arg("2");
        let started = Instant::now();
        let error = run_command_configured(&mut command, Duration::from_millis(25)).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
