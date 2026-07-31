//! Minimal Linux perf-event support for sysfs-described energy PMUs.

use super::util::{list_entries, read_f64, read_trimmed, read_u64};
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::FromRawFd;
use std::path::Path;

const PERF_FLAG_FD_CLOEXEC: libc::c_ulong = 1 << 3;
const PERF_ATTR_SIZE_VER0: u32 = 64;

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
        // SAFETY: `attr` has the Linux perf_event_attr v0 layout, and all other
        // arguments are plain values accepted by perf_event_open(2).
        let fd = unsafe {
            libc::syscall(
                libc::SYS_perf_event_open,
                &attr as *const PerfEventAttr,
                -1_i32,
                cpu as i32,
                -1_i32,
                PERF_FLAG_FD_CLOEXEC,
            )
        };
        if fd < 0 {
            Err(io::Error::last_os_error())
        } else {
            // SAFETY: a successful perf_event_open call returns a new owned file descriptor.
            let file = unsafe { File::from_raw_fd(fd as i32) };
            Ok(PerfCounter {
                file,
                joules_per_count: self.joules_per_count,
            })
        }
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

    #[test]
    fn uses_the_version_zero_kernel_layout() {
        assert_eq!(
            std::mem::size_of::<PerfEventAttr>(),
            PERF_ATTR_SIZE_VER0 as usize
        );
    }
}
