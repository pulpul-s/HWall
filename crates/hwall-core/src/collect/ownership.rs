//! Maps hwmon drivers to the physical device that owns their telemetry.
//!
//! Sysfs ancestry describes transport, not necessarily ownership. For example,
//! a DIMM temperature sensor may be below an SMBus PCI function, but its values
//! belong to the memory module. Keeping this policy in one registry prevents
//! collectors and renderers from growing independent driver-specific branches.

use super::memory::{locator_from_sysfs, slot_device_id};
use super::util::{
    block_device_from_path, i2c_client_from_path, pci_address_from_path, stable_path_token,
};
use crate::model::{Device, DeviceClass, Identification, SensorKind};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PhysicalOwner {
    Cpu,
    Memory,
    Gpu,
    Storage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdentityStrategy {
    CpuPackage,
    MemoryI2cClient,
    DriverInstance,
    NvmeController,
    BlockDevice,
    PciFunction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TemperatureLabel {
    None,
    /// Use a semantic label only for the first temperature channel.
    First(&'static str),
    /// Preserve the channel number when firmware provides no label.
    Indexed(&'static str),
}

#[derive(Debug)]
pub(super) struct DriverProfile {
    canonical_name: &'static str,
    aliases: &'static [&'static str],
    owner: PhysicalOwner,
    identity: IdentityStrategy,
    display_name: &'static str,
    sensor_family: Option<&'static str>,
    memory_type: Option<&'static str>,
    memory_owner_name: Option<&'static str>,
    temperature_label: TemperatureLabel,
}

#[derive(Debug)]
pub(super) struct ResolvedDevice {
    pub id: String,
    pub class: DeviceClass,
    pub profile: Option<&'static DriverProfile>,
}

const DRIVER_PROFILES: &[DriverProfile] = &[
    DriverProfile {
        canonical_name: "cpu",
        aliases: &[
            "coretemp",
            "k8temp",
            "k10temp",
            "zenpower",
            "fam15h_power",
            "peci_cputemp",
        ],
        owner: PhysicalOwner::Cpu,
        identity: IdentityStrategy::CpuPackage,
        display_name: "CPU sensor controller",
        sensor_family: None,
        memory_type: None,
        memory_owner_name: None,
        temperature_label: TemperatureLabel::None,
    },
    DriverProfile {
        canonical_name: "spd5118",
        aliases: &["spd5118"],
        owner: PhysicalOwner::Memory,
        identity: IdentityStrategy::MemoryI2cClient,
        display_name: "SPD5118 DDR5 memory module",
        sensor_family: Some("SPD5118"),
        memory_type: Some("DDR5"),
        memory_owner_name: Some("DDR5 module"),
        temperature_label: TemperatureLabel::First("Module temperature"),
    },
    DriverProfile {
        canonical_name: "jc42",
        aliases: &["jc42"],
        owner: PhysicalOwner::Memory,
        identity: IdentityStrategy::MemoryI2cClient,
        display_name: "JC42 memory temperature sensor",
        sensor_family: Some("JC42"),
        memory_type: None,
        memory_owner_name: Some("Memory module"),
        temperature_label: TemperatureLabel::First("Module temperature"),
    },
    DriverProfile {
        canonical_name: "peci_dimmtemp",
        aliases: &["peci_dimmtemp"],
        owner: PhysicalOwner::Memory,
        identity: IdentityStrategy::DriverInstance,
        display_name: "PECI DIMM temperature sensors",
        sensor_family: Some("PECI DIMM"),
        memory_type: None,
        memory_owner_name: Some("DIMM"),
        temperature_label: TemperatureLabel::Indexed("DIMM temperature"),
    },
    DriverProfile {
        canonical_name: "nvme",
        aliases: &["nvme"],
        owner: PhysicalOwner::Storage,
        identity: IdentityStrategy::NvmeController,
        display_name: "NVMe sensor controller",
        sensor_family: None,
        memory_type: None,
        memory_owner_name: None,
        temperature_label: TemperatureLabel::First("Controller temperature"),
    },
    DriverProfile {
        canonical_name: "drivetemp",
        aliases: &["drivetemp"],
        owner: PhysicalOwner::Storage,
        identity: IdentityStrategy::BlockDevice,
        display_name: "Drive temperature sensor controller",
        sensor_family: None,
        memory_type: None,
        memory_owner_name: None,
        temperature_label: TemperatureLabel::First("Drive temperature"),
    },
    DriverProfile {
        canonical_name: "gpu",
        aliases: &["amdgpu", "radeon", "nouveau", "nvidia", "i915", "xe"],
        owner: PhysicalOwner::Gpu,
        identity: IdentityStrategy::PciFunction,
        display_name: "GPU sensor controller",
        sensor_family: None,
        memory_type: None,
        memory_owner_name: None,
        temperature_label: TemperatureLabel::First("GPU temperature"),
    },
];

pub(super) fn resolve_device(path: &Path, driver_name: Option<&str>) -> ResolvedDevice {
    if let Some(profile) = driver_name.and_then(profile_for) {
        let id = profile
            .device_id(path)
            .unwrap_or_else(|| profile.fallback_device_id(path));
        return ResolvedDevice {
            id,
            class: profile.device_class(),
            profile: Some(profile),
        };
    }

    if let Some(address) = pci_address_from_path(path) {
        return ResolvedDevice {
            id: format!("pci:{address}"),
            class: DeviceClass::Pci,
            profile: None,
        };
    }

    if let Some(device_name) = block_device_from_path(path) {
        return ResolvedDevice {
            id: format!("block:{device_name}"),
            class: DeviceClass::Storage,
            profile: None,
        };
    }

    ResolvedDevice {
        id: format!("sysfs:{}", stable_path_token(path)),
        class: DeviceClass::Other,
        profile: None,
    }
}

pub(super) fn configure_hwmon_device(
    device: &mut Device,
    profile: Option<&DriverProfile>,
    device_path: &Path,
) {
    let Some(profile) = profile else {
        return;
    };

    insert_static_property(device, "sensor_family", profile.sensor_family);
    insert_static_property(device, "memory_type", profile.memory_type);
    insert_static_property(device, "memory_owner_name", profile.memory_owner_name);

    if profile.owner == PhysicalOwner::Memory
        && let Some(locator) = locator_from_sysfs(device_path)
    {
        device
            .properties
            .insert("locator".to_owned(), locator.into());
    }

    if profile.identity == IdentityStrategy::MemoryI2cClient {
        add_i2c_identity(device, device_path);
    }
}

pub(super) fn hwmon_display_name(
    driver_name: &str,
    profile: Option<&DriverProfile>,
    device_path: &Path,
) -> String {
    let Some(profile) = profile else {
        return format!(
            "{} sensor controller",
            super::util::humanize_token(driver_name)
        );
    };

    if profile.owner == PhysicalOwner::Memory
        && let Some(locator) = locator_from_sysfs(device_path)
    {
        return format!("{} — {locator}", profile.family_name());
    }

    profile.display_name.to_owned()
}

pub(super) fn default_sensor_label(
    profile: Option<&DriverProfile>,
    kind: &SensorKind,
    channel: Option<u32>,
    is_average: bool,
) -> Option<(String, Identification)> {
    let profile = profile?;
    if kind != &SensorKind::Temperature {
        return None;
    }

    let label = match profile.temperature_label {
        TemperatureLabel::None => return None,
        TemperatureLabel::First(label) if channel == Some(1) => label.to_owned(),
        TemperatureLabel::First(_) => return None,
        TemperatureLabel::Indexed(label) => match channel {
            Some(channel) => format!("{label} {channel}"),
            None => label.to_owned(),
        },
    };

    let label = if is_average {
        format!("{label} average")
    } else {
        label
    };
    Some((label, Identification::KnownDriverMapping))
}

fn profile_for(driver_name: &str) -> Option<&'static DriverProfile> {
    let normalized = normalize_driver_name(driver_name);
    DRIVER_PROFILES.iter().find(|profile| {
        profile
            .aliases
            .iter()
            .any(|alias| driver_name_matches(&normalized, alias))
    })
}

fn driver_name_matches(driver_name: &str, alias: &str) -> bool {
    driver_name == alias
        || driver_name
            .strip_prefix(alias)
            .is_some_and(|suffix| suffix.starts_with('.') || suffix.starts_with('_'))
}

fn normalize_driver_name(driver_name: &str) -> String {
    driver_name.trim().to_ascii_lowercase().replace('-', "_")
}

fn insert_static_property(device: &mut Device, key: &str, value: Option<&'static str>) {
    if let Some(value) = value {
        device.properties.insert(key.to_owned(), value.into());
    }
}

fn add_i2c_identity(device: &mut Device, device_path: &Path) {
    let Some(client) = i2c_client_from_path(device_path) else {
        return;
    };

    device.bus_address = Some(client.clone());
    let Some((bus, address)) = client.split_once('-') else {
        return;
    };

    device
        .properties
        .insert("i2c_bus".to_owned(), bus.to_owned().into());
    if let Ok(address) = u16::from_str_radix(address, 16) {
        device
            .properties
            .insert("i2c_address".to_owned(), format!("0x{address:02x}").into());
    }
}

impl DriverProfile {
    fn device_class(&self) -> DeviceClass {
        match self.owner {
            PhysicalOwner::Cpu => DeviceClass::Cpu,
            PhysicalOwner::Memory => DeviceClass::Memory,
            PhysicalOwner::Gpu => DeviceClass::Gpu,
            PhysicalOwner::Storage => DeviceClass::Storage,
        }
    }

    fn device_id(&self, path: &Path) -> Option<String> {
        match self.identity {
            IdentityStrategy::CpuPackage => Some("cpu:0".to_owned()),
            IdentityStrategy::MemoryI2cClient => {
                if let Some(locator) = locator_from_sysfs(path)
                    && let Some(id) = slot_device_id(&locator)
                {
                    return Some(id);
                }
                i2c_client_from_path(path)
                    .map(|client| format!("memory:{}:{client}", self.canonical_name))
            }
            IdentityStrategy::DriverInstance => None,
            IdentityStrategy::NvmeController => {
                nvme_controller_from_path(path).map(|controller| format!("block:{controller}"))
            }
            IdentityStrategy::BlockDevice => {
                block_device_from_path(path).map(|name| format!("block:{name}"))
            }
            IdentityStrategy::PciFunction => {
                pci_address_from_path(path).map(|address| format!("pci:{address}"))
            }
        }
    }

    fn fallback_device_id(&self, path: &Path) -> String {
        format!(
            "{}:{}:{}",
            self.owner_namespace(),
            self.canonical_name,
            stable_path_token(path)
        )
    }

    fn owner_namespace(&self) -> &'static str {
        match self.owner {
            PhysicalOwner::Cpu => "cpu",
            PhysicalOwner::Memory => "memory",
            PhysicalOwner::Gpu => "gpu",
            PhysicalOwner::Storage => "block",
        }
    }

    fn family_name(&self) -> &'static str {
        self.sensor_family.unwrap_or(self.canonical_name)
    }
}

fn nvme_controller_from_path(path: &Path) -> Option<String> {
    path.components()
        .filter_map(|component| component.as_os_str().to_str())
        .filter_map(nvme_controller_name)
        .next_back()
}

fn nvme_controller_name(name: &str) -> Option<String> {
    let suffix = name.strip_prefix("nvme")?;
    let controller_digits = suffix
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    (!controller_digits.is_empty()).then(|| format!("nvme{controller_digits}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_profiles_ignore_the_upstream_pci_controller() {
        let path = Path::new("/sys/devices/pci0000:00/0000:00:14.0/i2c-6/6-0053");
        let resolved = resolve_device(path, Some("spd5118"));
        assert_eq!(resolved.id, "memory:spd5118:6-0053");
        assert_eq!(resolved.class, DeviceClass::Memory);

        let jc42 = Path::new("/sys/devices/platform/i2c-1/1-0018");
        let resolved = resolve_device(jc42, Some("jc42"));
        assert_eq!(resolved.id, "memory:jc42:1-0018");
        assert_eq!(resolved.class, DeviceClass::Memory);
    }

    #[test]
    fn known_profiles_keep_their_owner_when_transport_identity_is_missing() {
        let path = Path::new("/sys/devices/platform/unusual-memory-sensor");
        let resolved = resolve_device(path, Some("jc42"));
        assert!(resolved.id.starts_with("memory:jc42:"));
        assert_eq!(resolved.class, DeviceClass::Memory);
    }

    #[test]
    fn recognizes_driver_name_variants() {
        assert!(profile_for("peci-dimmtemp").is_some());
        assert!(profile_for("peci_dimmtemp.cpu0").is_some());
        assert!(profile_for("i915").is_some());
    }

    #[test]
    fn maps_storage_and_gpu_profiles_to_their_physical_devices() {
        let nvme = Path::new("/sys/devices/pci0000:00/0000:02:00.0/nvme/nvme2");
        let resolved = resolve_device(nvme, Some("nvme"));
        assert_eq!(resolved.id, "block:nvme2");
        assert_eq!(resolved.class, DeviceClass::Storage);

        let gpu = Path::new("/sys/devices/pci0000:00/0000:01:00.0");
        let resolved = resolve_device(gpu, Some("amdgpu"));
        assert_eq!(resolved.id, "pci:0000:01:00.0");
        assert_eq!(resolved.class, DeviceClass::Gpu);
    }

    #[test]
    fn fixed_labels_do_not_hide_additional_temperature_channels() {
        let profile = profile_for("spd5118");
        assert_eq!(
            default_sensor_label(profile, &SensorKind::Temperature, Some(1), false)
                .map(|value| value.0)
                .as_deref(),
            Some("Module temperature")
        );
        assert!(default_sensor_label(profile, &SensorKind::Temperature, Some(2), false).is_none());
    }
}
