use super::util::{
    add_bool, add_string, add_u64, basename, is_virtual_block_device_name, list_dirs, read_bool01,
    read_trimmed, read_u64, symlink_basename,
};
use crate::model::{Device, DeviceClass, SnapshotBuilder};

pub(super) fn collect(builder: &mut SnapshotBuilder, include_sensitive: bool) {
    for path in list_dirs("/sys/class/block") {
        let Some(name) = basename(&path) else {
            continue;
        };
        let is_partition = path.join("partition").exists();
        let model = read_trimmed(path.join("device/model"));
        let vendor = read_trimmed(path.join("device/vendor"));
        let display = match (&vendor, &model) {
            (Some(vendor), Some(model)) => format!("{} {}", vendor.trim(), model.trim()),
            (_, Some(model)) => model.trim().to_owned(),
            _ => name.clone(),
        };
        let mut device = Device::new(format!("block:{name}"), DeviceClass::Storage, display);
        device.vendor = vendor.map(|value| value.trim().to_owned());
        device.model = model.map(|value| value.trim().to_owned());
        device.bus_address = Some(name.clone());
        device.driver = symlink_basename(path.join("device/driver"));
        if is_partition {
            if let Some(parent) = partition_parent(&name) {
                device.parent = Some(format!("block:{parent}"));
            }
        } else if let Some(controller) = nvme_controller_parent(&name) {
            device.parent = Some(format!("block:{controller}"));
        }
        let sectors = read_u64(path.join("size"));
        add_u64(
            &mut device.properties,
            "capacity_bytes",
            sectors.and_then(|value| value.checked_mul(512)),
        );
        add_bool(&mut device.properties, "partition", Some(is_partition));
        add_bool(
            &mut device.properties,
            "rotational",
            read_bool01(path.join("queue/rotational")),
        );
        add_bool(
            &mut device.properties,
            "removable",
            read_bool01(path.join("removable")),
        );
        add_u64(
            &mut device.properties,
            "logical_block_size",
            read_u64(path.join("queue/logical_block_size")),
        );
        add_u64(
            &mut device.properties,
            "physical_block_size",
            read_u64(path.join("queue/physical_block_size")),
        );
        add_u64(
            &mut device.properties,
            "minimum_io_size",
            read_u64(path.join("queue/minimum_io_size")),
        );
        add_u64(
            &mut device.properties,
            "optimal_io_size",
            read_u64(path.join("queue/optimal_io_size")),
        );
        add_string(
            &mut device.properties,
            "scheduler",
            read_trimmed(path.join("queue/scheduler")),
        );
        add_string(
            &mut device.properties,
            "firmware_revision",
            read_trimmed(path.join("device/rev")),
        );
        if include_sensitive {
            add_string(
                &mut device.properties,
                "wwid",
                read_trimmed(path.join("wwid")),
            );
        }
        if include_sensitive {
            add_string(
                &mut device.properties,
                "serial",
                read_trimmed(path.join("device/serial")),
            );
        }
        add_activity_counters(&mut device, &path, is_partition, &name);
        builder.add_device(device);
    }
}

fn partition_parent(name: &str) -> Option<String> {
    if name.starts_with("nvme") || name.starts_with("mmcblk") {
        let index = name.rfind('p')?;
        return Some(name[..index].to_owned());
    }
    let index = name.find(|c: char| c.is_ascii_digit())?;
    Some(name[..index].to_owned())
}

fn nvme_controller_parent(name: &str) -> Option<String> {
    if !name.starts_with("nvme") {
        return None;
    }
    let suffix = &name[4..];
    let marker = suffix.find('n')?;
    Some(name[..4 + marker].to_owned())
}

pub(super) fn collect_dynamic(builder: &mut SnapshotBuilder) {
    let paths = list_dirs("/sys/class/block");
    let names: Vec<String> = paths.iter().filter_map(basename).collect();

    for path in paths {
        let Some(name) = basename(&path) else {
            continue;
        };
        let is_partition = path.join("partition").exists();
        let device_id = activity_device_id(&name, &names);
        let mut device = Device::new(device_id, DeviceClass::Storage, name.clone());
        add_bool(&mut device.properties, "partition", Some(is_partition));
        add_activity_counters(&mut device, &path, is_partition, &name);
        if !device.counters.is_empty() {
            builder.add_device(device);
        }
    }
}

fn activity_device_id(name: &str, block_names: &[String]) -> String {
    let Some(controller) = nvme_controller_parent(name) else {
        return format!("block:{name}");
    };
    let namespace_count = block_names
        .iter()
        .filter(|candidate| {
            !candidate.contains('p')
                && nvme_controller_parent(candidate).as_deref() == Some(&controller)
        })
        .count();
    if namespace_count == 1 {
        format!("block:{controller}")
    } else {
        format!("block:{name}")
    }
}

fn add_activity_counters(
    device: &mut Device,
    path: &std::path::Path,
    is_partition: bool,
    name: &str,
) {
    if is_partition || is_virtual_block_device_name(name) || !path.join("device").exists() {
        return;
    }
    let Some(stat) = read_trimmed(path.join("stat")) else {
        return;
    };
    let Ok(fields) = stat
        .split_whitespace()
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
    else {
        return;
    };
    if fields.len() < 10 {
        return;
    }
    for (key, index) in [
        ("read_operations", 0usize),
        ("read_sectors", 2usize),
        ("write_operations", 4usize),
        ("write_sectors", 6usize),
        ("io_milliseconds", 9usize),
    ] {
        if let Some(value) = fields.get(index) {
            device.counters.insert(key.to_owned(), *value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::activity_device_id;

    #[test]
    fn single_nvme_namespace_uses_controller_identity() {
        let names = vec!["nvme0n1".to_owned(), "sda".to_owned()];
        assert_eq!(activity_device_id("nvme0n1", &names), "block:nvme0");
    }

    #[test]
    fn multiple_nvme_namespaces_keep_namespace_identity() {
        let names = vec!["nvme0n1".to_owned(), "nvme0n2".to_owned()];
        assert_eq!(activity_device_id("nvme0n1", &names), "block:nvme0n1");
    }
}
