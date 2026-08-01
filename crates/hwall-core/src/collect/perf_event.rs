//! Minimal Linux perf-event support for sysfs-described PMUs.

use super::util::{list_entries, read_f64, read_trimmed, read_u64};
use std::fs::File;
use std::io::{self, Read};
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
use std::os::fd::AsRawFd;
use std::os::fd::FromRawFd;
use std::path::Path;

const PERF_FLAG_FD_CLOEXEC: libc::c_ulong = 1 << 3;
const PERF_ATTR_SIZE_VER0: u32 = 64;
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
const PERF_FORMAT_TOTAL_TIME_ENABLED: u64 = 1 << 0;
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
const PERF_FORMAT_TOTAL_TIME_RUNNING: u64 = 1 << 1;
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
const PERF_FORMAT_GROUP: u64 = 1 << 3;
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
const MAX_SYSFS_VALUE_BYTES: u64 = 4 * 1024;

#[derive(Debug, Clone)]
pub(super) struct PerfEvent {
    pub(super) name: String,
    event_type: u32,
    config: u64,
    joules_per_count: f64,
}

impl PerfEvent {
    pub(super) fn discover(pmu: &Path) -> Vec<Self> {
        let Some(event_type) =
            read_u64(pmu.join("type")).and_then(|value| u32::try_from(value).ok())
        else {
            return Vec::new();
        };
        let Some((config_start, config_width)) =
            read_trimmed(pmu.join("format/event")).and_then(|value| parse_config_range(&value))
        else {
            return Vec::new();
        };
        let events = pmu.join("events");

        list_entries(&events)
            .into_iter()
            .filter_map(|path| {
                let name = path.file_name()?.to_str()?;
                if !name.starts_with("energy-")
                    || name.ends_with(".scale")
                    || name.ends_with(".unit")
                {
                    return None;
                }
                let unit = read_trimmed(events.join(format!("{name}.unit")))?;
                if !unit.eq_ignore_ascii_case("joules") && !unit.eq_ignore_ascii_case("joule") {
                    return None;
                }
                let event = read_trimmed(&path).and_then(|value| parse_event_value(&value))?;
                if config_width < 64 && event >= (1_u64 << config_width) {
                    return None;
                }
                let joules_per_count = read_f64(events.join(format!("{name}.scale")))?;
                if !joules_per_count.is_finite() || joules_per_count <= 0.0 {
                    return None;
                }
                Some(Self {
                    name: name.to_owned(),
                    event_type,
                    config: event << config_start,
                    joules_per_count,
                })
            })
            .collect()
    }

    pub(super) fn open(&self, cpu: u32) -> io::Result<PerfCounter> {
        let attr = PerfEventAttr {
            event_type: self.event_type,
            size: PERF_ATTR_SIZE_VER0,
            config: self.config,
            ..PerfEventAttr::default()
        };
        Ok(PerfCounter {
            file: open_perf_event(&attr, cpu, -1)?,
            joules_per_count: self.joules_per_count,
        })
    }
}

#[derive(Debug)]
pub(super) struct PerfCounter {
    file: File,
    joules_per_count: f64,
}

impl PerfCounter {
    pub(super) fn read(&self) -> io::Result<(u64, f64)> {
        let mut bytes = [0_u8; std::mem::size_of::<u64>()];
        let mut file = &self.file;
        file.read_exact(&mut bytes)?;
        Ok((u64::from_ne_bytes(bytes), self.joules_per_count))
    }
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RawPerfEvent {
    event_type: u32,
    config: u64,
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
impl RawPerfEvent {
    pub(super) fn discover(pmu: &Path, event_name: &str) -> io::Result<Self> {
        let event_type = read_small_trimmed(&pmu.join("type"))?
            .parse::<u32>()
            .map_err(|_| invalid_data("invalid perf PMU type"))?;
        let (config_start, config_width) =
            parse_config_range(&read_small_trimmed(&pmu.join("format/event"))?)
                .ok_or_else(|| invalid_data("unsupported perf event format"))?;
        let event = parse_event_value(&read_small_trimmed(&pmu.join("events").join(event_name))?)
            .ok_or_else(|| invalid_data("invalid perf event value"))?;
        if config_width < 64 && event >= (1_u64 << config_width) {
            return Err(invalid_data("perf event value exceeds its config field"));
        }
        Ok(Self {
            event_type,
            config: event << config_start,
        })
    }
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[derive(Debug)]
pub(super) struct PerfCounterGroup {
    leader: File,
    _member: File,
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
impl PerfCounterGroup {
    pub(super) fn open(cpu: u32, leader: RawPerfEvent, member: RawPerfEvent) -> io::Result<Self> {
        let read_format =
            PERF_FORMAT_GROUP | PERF_FORMAT_TOTAL_TIME_ENABLED | PERF_FORMAT_TOTAL_TIME_RUNNING;
        let leader_attr = PerfEventAttr {
            event_type: leader.event_type,
            size: PERF_ATTR_SIZE_VER0,
            config: leader.config,
            read_format,
            ..PerfEventAttr::default()
        };
        let leader = open_perf_event(&leader_attr, cpu, -1)?;
        let member_attr = PerfEventAttr {
            event_type: member.event_type,
            size: PERF_ATTR_SIZE_VER0,
            config: member.config,
            read_format,
            ..PerfEventAttr::default()
        };
        let member = open_perf_event(&member_attr, cpu, leader.as_raw_fd())?;
        Ok(Self {
            leader,
            _member: member,
        })
    }

    pub(super) fn read(&self) -> io::Result<PerfGroupReading> {
        let mut leader = &self.leader;
        read_group_record(&mut leader)
    }
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PerfGroupReading {
    pub(super) first: u64,
    pub(super) second: u64,
    pub(super) time_enabled: u64,
    pub(super) time_running: u64,
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn read_group_record(reader: &mut impl Read) -> io::Result<PerfGroupReading> {
    const WORDS: usize = 5;
    let mut bytes = [0_u8; WORDS * std::mem::size_of::<u64>()];
    let bytes_read = reader.read(&mut bytes)?;
    if bytes_read != bytes.len() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "short perf group read",
        ));
    }
    decode_group_read(&bytes)
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn decode_group_read(bytes: &[u8]) -> io::Result<PerfGroupReading> {
    if bytes.len() != 5 * std::mem::size_of::<u64>() {
        return Err(invalid_data("invalid perf group read size"));
    }
    let mut words = [0_u64; 5];
    for (index, chunk) in bytes.chunks_exact(std::mem::size_of::<u64>()).enumerate() {
        words[index] = u64::from_ne_bytes(
            chunk
                .try_into()
                .map_err(|_| invalid_data("invalid perf group word"))?,
        );
    }
    if words[0] != 2 || words[2] > words[1] {
        return Err(invalid_data("invalid perf group contents"));
    }
    Ok(PerfGroupReading {
        time_enabled: words[1],
        time_running: words[2],
        first: words[3],
        second: words[4],
    })
}

fn open_perf_event(attr: &PerfEventAttr, cpu: u32, group_fd: i32) -> io::Result<File> {
    let cpu = i32::try_from(cpu).map_err(|_| invalid_data("CPU ID exceeds perf API limits"))?;
    // SAFETY: `attr` has the Linux perf_event_attr v0 layout, and all other
    // arguments are plain values accepted by perf_event_open(2).
    let fd = unsafe {
        libc::syscall(
            libc::SYS_perf_event_open,
            attr as *const PerfEventAttr,
            -1_i32,
            cpu,
            group_fd,
            PERF_FLAG_FD_CLOEXEC,
        )
    };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: a successful perf_event_open call returns a new owned file descriptor.
        Ok(unsafe { File::from_raw_fd(fd as i32) })
    }
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn read_small_trimmed(path: &Path) -> io::Result<String> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_SYSFS_VALUE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_SYSFS_VALUE_BYTES {
        return Err(invalid_data("sysfs value is unexpectedly large"));
    }
    let value = String::from_utf8(bytes).map_err(|_| invalid_data("sysfs value is not UTF-8"))?;
    let value = value.trim();
    if value.is_empty() {
        Err(invalid_data("sysfs value is empty"))
    } else {
        Ok(value.to_owned())
    }
}

fn invalid_data(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn parse_config_range(value: &str) -> Option<(u32, u32)> {
    let (register, range) = value.trim().split_once(':')?;
    if register != "config" || range.contains(',') {
        return None;
    }
    let (start, end) = range
        .split_once('-')
        .map_or((range, range), |(start, end)| (start, end));
    let start = start.parse::<u32>().ok()?;
    let end = end.parse::<u32>().ok()?;
    (start <= end && end < 64).then_some((start, end - start + 1))
}

fn parse_event_value(value: &str) -> Option<u64> {
    let mut assignments = value.split(',');
    let (name, value) = assignments.next()?.trim().split_once('=')?;
    if name != "event" || assignments.next().is_some() {
        return None;
    }
    let value = value.trim();
    if let Some(value) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        u64::from_str_radix(value, 16).ok()
    } else {
        value.parse().ok()
    }
}

#[repr(C)]
#[derive(Debug, Default)]
struct PerfEventAttr {
    event_type: u32,
    size: u32,
    config: u64,
    sample_period: u64,
    sample_type: u64,
    read_format: u64,
    flags: u64,
    wakeup_events: u32,
    breakpoint_type: u32,
    config1: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rapl_event_encoding() {
        assert_eq!(parse_config_range("config:0-7"), Some((0, 8)));
        assert_eq!(parse_event_value("event=0x2"), Some(2));
        assert_eq!(parse_event_value("event=3"), Some(3));
    }

    #[test]
    fn rejects_unsupported_event_encodings() {
        assert_eq!(parse_config_range("config1:0-7"), None);
        assert_eq!(parse_config_range("config:0-3,8-11"), None);
        assert_eq!(parse_event_value("event=3,other=1"), None);
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    #[test]
    fn decodes_grouped_perf_values() {
        let words = [2_u64, 1_000, 900, 41, 56];
        let bytes = words
            .into_iter()
            .flat_map(|word| word.to_ne_bytes())
            .collect::<Vec<_>>();
        assert_eq!(
            decode_group_read(&bytes).unwrap(),
            PerfGroupReading {
                first: 41,
                second: 56,
                time_enabled: 1_000,
                time_running: 900,
            }
        );
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    #[test]
    fn rejects_invalid_grouped_perf_values() {
        assert!(decode_group_read(&[0_u8; 8]).is_err());
        let invalid = [3_u64, 1_000, 900, 41, 56]
            .into_iter()
            .flat_map(|word| word.to_ne_bytes())
            .collect::<Vec<_>>();
        assert!(decode_group_read(&invalid).is_err());
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    #[test]
    fn rejects_a_short_group_record_without_a_second_read() {
        struct ShortReader {
            bytes: Vec<u8>,
            calls: usize,
        }

        impl Read for ShortReader {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                self.calls += 1;
                let count = self.bytes.len().min(buffer.len()).min(8);
                buffer[..count].copy_from_slice(&self.bytes[..count]);
                Ok(count)
            }
        }

        let mut reader = ShortReader {
            bytes: vec![0; 40],
            calls: 0,
        };
        let error = read_group_record(&mut reader).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        assert_eq!(reader.calls, 1);
    }

    #[test]
    fn uses_the_version_zero_kernel_layout() {
        assert_eq!(
            std::mem::size_of::<PerfEventAttr>(),
            PERF_ATTR_SIZE_VER0 as usize
        );
    }
}
