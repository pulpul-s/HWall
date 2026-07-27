use super::util::{add_string, add_u64, basename, list_dirs, read_trimmed, read_u64};
use crate::model::{
    Device, DeviceClass, Identification, Sensor, SensorKind, SnapshotBuilder, Unit,
};
use std::collections::BTreeMap;
use std::path::Path;

pub(super) fn collect(builder: &mut SnapshotBuilder) {
    collect_edac(builder);
}

pub(super) fn collect_usage(builder: &mut SnapshotBuilder) {
    let Ok(content) = std::fs::read_to_string("/proc/meminfo") else {
        return;
    };
    let values: BTreeMap<String, u64> = content
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            let kib = value.split_whitespace().next()?.parse::<u64>().ok()?;
            Some((key.to_owned(), kib.saturating_mul(1024)))
        })
        .collect();
    let Some(total) = values.get("MemTotal").copied() else {
        return;
    };
    let available = values
        .get("MemAvailable")
        .copied()
        .or_else(|| values.get("MemFree").copied())
        .unwrap_or(0)
        .min(total);
    let used = total.saturating_sub(available);
    let cached = values
        .get("Cached")
        .copied()
        .unwrap_or(0)
        .saturating_add(values.get("SReclaimable").copied().unwrap_or(0))
        .saturating_sub(values.get("Shmem").copied().unwrap_or(0));
    let buffers = values.get("Buffers").copied().unwrap_or(0);
    let swap_total = values.get("SwapTotal").copied().unwrap_or(0);
    let swap_free = values.get("SwapFree").copied().unwrap_or(0).min(swap_total);
    let swap_used = swap_total.saturating_sub(swap_free);
    let utilization = if total > 0 {
        Some(used as f64 * 100.0 / total as f64)
    } else {
        None
    };

    let mut device = Device::new("memory:system", DeviceClass::Memory, "System memory");
    device
        .properties
        .insert("memory_role".to_owned(), "system_usage".into());
    device
        .properties
        .insert("total_bytes".to_owned(), total.into());
    device.sensors.extend([
        Sensor::new(
            "memory:system:used",
            "Memory used",
            SensorKind::Capacity,
            Unit::Byte,
            Some(used as f64),
            "/proc/meminfo",
            Identification::Inferred,
        ),
        Sensor::new(
            "memory:system:available",
            "Memory available",
            SensorKind::Capacity,
            Unit::Byte,
            Some(available as f64),
            "/proc/meminfo",
            Identification::KernelLabel,
        ),
        Sensor::new(
            "memory:system:cached",
            "Memory cached",
            SensorKind::Capacity,
            Unit::Byte,
            Some(cached as f64),
            "/proc/meminfo",
            Identification::KernelLabel,
        ),
        Sensor::new(
            "memory:system:buffers",
            "Memory buffers",
            SensorKind::Capacity,
            Unit::Byte,
            Some(buffers as f64),
            "/proc/meminfo",
            Identification::KernelLabel,
        ),
        Sensor::new(
            "memory:system:utilization",
            "Memory utilization",
            SensorKind::Utilization,
            Unit::Percent,
            utilization,
            "/proc/meminfo",
            Identification::Inferred,
        ),
    ]);
    if swap_total > 0 {
        device.sensors.push(Sensor::new(
            "memory:system:swap_used",
            "Swap used",
            SensorKind::Capacity,
            Unit::Byte,
            Some(swap_used as f64),
            "/proc/meminfo",
            Identification::Inferred,
        ));
        device.sensors.push(Sensor::new(
            "memory:system:swap_utilization",
            "Swap utilization",
            SensorKind::Utilization,
            Unit::Percent,
            Some(swap_used as f64 * 100.0 / swap_total as f64),
            "/proc/meminfo",
            Identification::Inferred,
        ));
    }
    builder.add_device(device);
}

pub(super) fn locator_from_sysfs(device_path: &Path) -> Option<String> {
    for key in [
        "dimm_label",
        "label",
        "slot",
        "location",
        "physical_location",
    ] {
        if let Some(value) =
            read_trimmed(device_path.join(key)).filter(|value| locator_is_specific(value))
        {
            return Some(value);
        }
    }

    let firmware_node = device_path.join("of_node");
    for key in ["label", "slot", "location"] {
        if let Some(value) =
            read_trimmed(firmware_node.join(key)).filter(|value| locator_is_specific(value))
        {
            return Some(value);
        }
    }

    None
}

pub(super) fn slot_device_id(locator: &str) -> Option<String> {
    if !locator_is_specific(locator) {
        return None;
    }
    let slug = locator_slug(locator);
    (!slug.is_empty()).then(|| format!("memory:slot:{slug}"))
}

pub(super) fn locator_is_specific(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.is_empty()
        || matches!(
            normalized.as_str(),
            "unknown"
                | "none"
                | "not specified"
                | "not installed"
                | "dimm"
                | "module"
                | "memory module"
                | "spd5118"
                | "jc42"
        )
    {
        return false;
    }

    let compact = normalized
        .chars()
        .filter(|character| !matches!(character, ' ' | '-' | '_'))
        .collect::<String>();
    if compact.is_empty() {
        return false;
    }

    !compact.strip_prefix("dimm").is_some_and(|suffix| {
        !suffix.is_empty() && suffix.chars().all(|character| character.is_ascii_digit())
    })
}

fn locator_slug(locator: &str) -> String {
    let mut slug = String::new();
    let mut previous_was_separator = false;

    for character in locator.trim().chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
            previous_was_separator = false;
        } else if !previous_was_separator && !slug.is_empty() {
            slug.push('-');
            previous_was_separator = true;
        }
    }

    slug.trim_matches('-').to_owned()
}

fn collect_edac(builder: &mut SnapshotBuilder) {
    for controller_path in list_dirs("/sys/devices/system/edac/mc") {
        let Some(controller_name) = basename(&controller_path) else {
            continue;
        };
        if !controller_name.starts_with("mc") {
            continue;
        }

        let controller_id = format!("memory:{controller_name}");
        let mut controller = Device::new(
            controller_id.clone(),
            DeviceClass::Memory,
            format!("Memory controller {controller_name}"),
        );
        controller
            .properties
            .insert("memory_role".to_owned(), "controller".into());
        controller
            .properties
            .insert("inventory_source".to_owned(), "edac".into());
        add_string(
            &mut controller.properties,
            "name",
            read_trimmed(controller_path.join("mc_name")),
        );
        add_u64(
            &mut controller.properties,
            "correctable_errors",
            read_u64(controller_path.join("ce_count")),
        );
        add_u64(
            &mut controller.properties,
            "uncorrectable_errors",
            read_u64(controller_path.join("ue_count")),
        );
        add_u64(
            &mut controller.properties,
            "size_mb",
            read_u64(controller_path.join("size_mb")),
        );
        builder.add_device(controller);

        for dimm_path in list_dirs(&controller_path) {
            let Some(dimm_name) = basename(&dimm_path) else {
                continue;
            };
            if !dimm_name.starts_with("dimm") {
                continue;
            }

            let label = read_trimmed(dimm_path.join("dimm_label"));
            let device_id = label
                .as_deref()
                .and_then(slot_device_id)
                .unwrap_or_else(|| format!("memory:{controller_name}:{dimm_name}"));
            let display_name = label
                .as_deref()
                .filter(|value| locator_is_specific(value))
                .map(|value| format!("Memory module {value}"))
                .unwrap_or_else(|| format!("Memory module {controller_name}/{dimm_name}"));

            let mut device = Device::new(device_id, DeviceClass::Memory, display_name);
            device.parent = Some(controller_id.clone());
            device
                .properties
                .insert("memory_role".to_owned(), "module".into());
            device
                .properties
                .insert("inventory_source".to_owned(), "edac".into());
            if let Some(label) = label.filter(|value| locator_is_specific(value)) {
                device.properties.insert("locator".to_owned(), label.into());
            }
            add_string(
                &mut device.properties,
                "memory_type",
                read_trimmed(dimm_path.join("dimm_mem_type")),
            );
            add_string(
                &mut device.properties,
                "device_type",
                read_trimmed(dimm_path.join("dimm_dev_type")),
            );
            add_string(
                &mut device.properties,
                "edac_mode",
                read_trimmed(dimm_path.join("dimm_edac_mode")),
            );
            add_u64(
                &mut device.properties,
                "size_mb",
                read_u64(dimm_path.join("size")),
            );
            add_u64(
                &mut device.properties,
                "correctable_errors",
                read_u64(dimm_path.join("dimm_ce_count")),
            );
            add_u64(
                &mut device.properties,
                "uncorrectable_errors",
                read_u64(dimm_path.join("dimm_ue_count")),
            );
            builder.add_device(device);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_physical_slot_names() {
        assert!(locator_is_specific("DIMM_A2"));
        assert!(locator_is_specific("CPU0 Channel1 DIMM0"));
        assert!(!locator_is_specific("DIMM"));
        assert!(!locator_is_specific("dimm0"));
        assert!(!locator_is_specific("Unknown"));
        assert!(!locator_is_specific("SPD5118"));
        assert!(!locator_is_specific("---"));
    }

    #[test]
    fn creates_stable_slot_ids() {
        assert_eq!(
            slot_device_id("DIMM A2").as_deref(),
            Some("memory:slot:dimm-a2")
        );
    }
}
