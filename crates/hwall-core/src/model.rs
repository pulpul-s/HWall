use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum DeviceClass {
    System,
    Motherboard,
    Bios,
    Cpu,
    Memory,
    Gpu,
    Storage,
    Network,
    Usb,
    Pci,
    PowerSupply,
    Battery,
    Thermal,
    SensorController,
    Thunderbolt,
    Other,
}

impl DeviceClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Motherboard => "motherboard",
            Self::Bios => "bios",
            Self::Cpu => "cpu",
            Self::Memory => "memory",
            Self::Gpu => "gpu",
            Self::Storage => "storage",
            Self::Network => "network",
            Self::Usb => "usb",
            Self::Pci => "pci",
            Self::PowerSupply => "power_supply",
            Self::Battery => "battery",
            Self::Thermal => "thermal",
            Self::SensorController => "sensor_controller",
            Self::Thunderbolt => "thunderbolt",
            Self::Other => "other",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Motherboard => "Motherboard",
            Self::Bios => "BIOS/firmware",
            Self::Cpu => "CPU",
            Self::Memory => "Memory",
            Self::Gpu => "GPU",
            Self::Storage => "Storage",
            Self::Network => "Network",
            Self::Usb => "USB",
            Self::Pci => "PCI",
            Self::PowerSupply => "Power supply",
            Self::Battery => "Battery",
            Self::Thermal => "Thermal",
            Self::SensorController => "Sensor controller",
            Self::Thunderbolt => "Thunderbolt/USB4",
            Self::Other => "Other",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum SensorKind {
    Temperature,
    Voltage,
    Current,
    Power,
    Energy,
    Fan,
    Frequency,
    Throughput,
    Utilization,
    Capacity,
    Humidity,
    Counter,
    Boolean,
    Other,
}

impl SensorKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Temperature => "temperature",
            Self::Voltage => "voltage",
            Self::Current => "current",
            Self::Power => "power",
            Self::Energy => "energy",
            Self::Fan => "fan",
            Self::Frequency => "frequency",
            Self::Throughput => "throughput",
            Self::Utilization => "utilization",
            Self::Capacity => "capacity",
            Self::Humidity => "humidity",
            Self::Counter => "counter",
            Self::Boolean => "boolean",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    Celsius,
    Volt,
    Ampere,
    Watt,
    WattHour,
    AmpereHour,
    Joule,
    Rpm,
    Hertz,
    Percent,
    Byte,
    BytePerSecond,
    Count,
    CountPerSecond,
    Boolean,
    Raw,
}

impl Unit {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Celsius => "celsius",
            Self::Volt => "volt",
            Self::Ampere => "ampere",
            Self::Watt => "watt",
            Self::WattHour => "watt_hour",
            Self::AmpereHour => "ampere_hour",
            Self::Joule => "joule",
            Self::Rpm => "rpm",
            Self::Hertz => "hertz",
            Self::Percent => "percent",
            Self::Byte => "byte",
            Self::BytePerSecond => "byte_per_second",
            Self::Count => "count",
            Self::CountPerSecond => "count_per_second",
            Self::Boolean => "boolean",
            Self::Raw => "raw",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Identification {
    KernelLabel,
    FirmwareLabel,
    LibSensorsConfig,
    VendorApi,
    KnownDriverMapping,
    BoardDatabase,
    Inferred,
    Unidentified,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReadingFreshness {
    #[default]
    Current,
    Stale,
    Unavailable,
}

impl ReadingFreshness {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Stale => "stale",
            Self::Unavailable => "unavailable",
        }
    }

    pub const fn is_current(self) -> bool {
        matches!(self, Self::Current)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum CollectorId {
    Cpu,
    Memory,
    Network,
    Block,
    Power,
    Thermal,
    Hwmon,
    LibSensors,
    Drm,
    NvidiaSmi,
    Energy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SensorStatus {
    Ok,
    Alarm,
    Fault,
    Unavailable,
}

impl SensorStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Alarm => "alarm",
            Self::Fault => "fault",
            Self::Unavailable => "unavailable",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Alarm => "Alarm",
            Self::Fault => "Fault",
            Self::Unavailable => "Unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StorageHealthStatus {
    Passed,
    Warning,
    Failed,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StorageHealthAvailability {
    Current,
    DriveAsleep,
    PermissionDenied,
    HelperMissing,
    Unsupported,
    Error,
}

impl StorageHealthStatus {
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Passed => "Passed",
            Self::Warning => "Warning",
            Self::Failed => "Failed",
            Self::Unknown => "Unknown",
        }
    }
}

impl StorageHealthAvailability {
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Current => "Current",
            Self::DriveAsleep => "Drive asleep",
            Self::PermissionDenied => "Permission required",
            Self::HelperMissing => "Helper unavailable",
            Self::Unsupported => "Unsupported",
            Self::Error => "Error",
        }
    }
}

impl std::fmt::Display for StorageHealthStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.display_name())
    }
}

impl std::fmt::Display for StorageHealthAvailability {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.display_name())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StorageHealth {
    pub status: StorageHealthStatus,
    pub availability: StorageHealthAvailability,
    pub last_attempt_unix_ms: Option<u128>,
    pub last_success_unix_ms: Option<u128>,
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
}

pub const STORAGE_HEALTH_PROPERTY_KEYS: &[&str] = &[
    "critical_warning",
    "critical_warning_details",
    "available_spare",
    "spare_threshold",
    "percentage_used",
    "power_on_hours",
    "power_cycles",
    "unsafe_shutdowns",
    "media_errors",
    "error_log_entries",
    "data_units_read",
    "data_units_written",
    "host_read_commands",
    "host_write_commands",
    "warning_temperature_time",
    "critical_temperature_time",
];

pub fn is_storage_health_property(key: &str) -> bool {
    STORAGE_HEALTH_PROPERTY_KEYS.contains(&key)
}

impl StorageHealth {
    pub fn needs_refresh(&self, now_unix_ms: u128, maximum_age_ms: u128) -> bool {
        match self.availability {
            StorageHealthAvailability::PermissionDenied
            | StorageHealthAvailability::HelperMissing
            | StorageHealthAvailability::Unsupported => false,
            StorageHealthAvailability::DriveAsleep | StorageHealthAvailability::Error => self
                .last_attempt_unix_ms
                .is_none_or(|attempted| now_unix_ms.saturating_sub(attempted) >= maximum_age_ms),
            StorageHealthAvailability::Current => self
                .last_success_unix_ms
                .is_none_or(|checked| now_unix_ms.saturating_sub(checked) >= maximum_age_ms),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum PropertyValue {
    String(String),
    Integer(i64),
    Unsigned(u64),
    Float(f64),
    Boolean(bool),
    Strings(Vec<String>),
}

impl PropertyValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Boolean(value) => Some(*value),
            Self::String(value) => match value.trim().to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" | "enabled" => Some(true),
                "0" | "false" | "no" | "off" | "disabled" => Some(false),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Unsigned(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Integer(value) => Some(*value as f64),
            Self::Unsigned(value) => Some(*value as f64),
            Self::Float(value) => Some(*value),
            Self::String(value) => value.parse().ok(),
            _ => None,
        }
    }
}

impl From<String> for PropertyValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for PropertyValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<u64> for PropertyValue {
    fn from(value: u64) -> Self {
        Self::Unsigned(value)
    }
}

impl From<i64> for PropertyValue {
    fn from(value: i64) -> Self {
        Self::Integer(value)
    }
}

impl From<f64> for PropertyValue {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}

impl From<bool> for PropertyValue {
    fn from(value: bool) -> Self {
        Self::Boolean(value)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Sensor {
    pub id: String,
    pub label: String,
    pub kind: SensorKind,
    pub unit: Unit,
    pub value: Option<f64>,
    pub raw_value: Option<String>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub critical: Option<f64>,
    pub status: SensorStatus,
    #[serde(default)]
    pub freshness: ReadingFreshness,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_updated_unix_ms: Option<u128>,
    pub source: String,
    pub identification: Identification,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, PropertyValue>,
    #[serde(skip)]
    pub(crate) collector: Option<CollectorId>,
}

impl Sensor {
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        kind: SensorKind,
        unit: Unit,
        value: Option<f64>,
        source: impl Into<String>,
        identification: Identification,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            kind,
            unit,
            value,
            raw_value: None,
            min: None,
            max: None,
            critical: None,
            status: SensorStatus::Ok,
            freshness: ReadingFreshness::Current,
            last_updated_unix_ms: None,
            source: source.into(),
            identification,
            metadata: BTreeMap::new(),
            collector: None,
        }
    }

    pub fn is_intermittent(&self) -> bool {
        matches!(
            self.metadata.get("intermittent"),
            Some(PropertyValue::Boolean(true))
        )
    }

    pub fn sampled_at_unix_ms(&self) -> Option<u128> {
        if !self.is_intermittent() {
            return None;
        }
        match self.metadata.get("sampled_at_unix_ms") {
            Some(PropertyValue::Unsigned(value)) => Some(u128::from(*value)),
            Some(PropertyValue::String(value)) => value.parse().ok(),
            _ => None,
        }
    }

    pub fn metadata_str(&self, key: &str) -> Option<&str> {
        self.metadata.get(key).and_then(PropertyValue::as_str)
    }

    pub fn metadata_u64(&self, key: &str) -> Option<u64> {
        self.metadata.get(key).and_then(PropertyValue::as_u64)
    }

    pub fn has_unconfigured_hardware_alarm(&self) -> bool {
        self.metadata.contains_key("alarm_note")
    }

    pub fn is_current(&self) -> bool {
        self.freshness.is_current()
    }

    pub(crate) fn mark_collector(&mut self, collector: CollectorId) {
        self.collector = Some(collector);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Device {
    pub id: String,
    pub parent: Option<String>,
    pub class: DeviceClass,
    pub name: String,
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub driver: Option<String>,
    pub bus_address: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, PropertyValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sensors: Vec<Sensor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_health: Option<StorageHealth>,
    #[serde(skip)]
    pub(crate) counters: BTreeMap<String, u64>,
}

impl Device {
    pub fn new(id: impl Into<String>, class: DeviceClass, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            parent: None,
            class,
            name: name.into(),
            vendor: None,
            model: None,
            driver: None,
            bus_address: None,
            properties: BTreeMap::new(),
            sensors: Vec::new(),
            storage_health: None,
            counters: BTreeMap::new(),
        }
    }

    pub fn property_str(&self, key: &str) -> Option<&str> {
        self.properties.get(key).and_then(PropertyValue::as_str)
    }

    pub fn property_bool(&self, key: &str) -> Option<bool> {
        self.properties.get(key).and_then(PropertyValue::as_bool)
    }

    fn merge_identity_from(&mut self, incoming: &mut Self) {
        replace_if_some(&mut self.vendor, &mut incoming.vendor);
        replace_if_some(&mut self.model, &mut incoming.model);
        replace_if_some(&mut self.driver, &mut incoming.driver);
        replace_if_some(&mut self.bus_address, &mut incoming.bus_address);
        self.properties.append(&mut incoming.properties);
        replace_if_some(&mut self.storage_health, &mut incoming.storage_health);
        self.counters.append(&mut incoming.counters);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Snapshot {
    pub schema_version: u32,
    pub captured_at_unix_ms: u128,
    pub devices: Vec<Device>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(skip)]
    pub(crate) successful_collectors: BTreeSet<CollectorId>,
    #[serde(skip)]
    pub(crate) failed_collectors: BTreeSet<CollectorId>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self::new()
    }
}

impl Snapshot {
    pub fn new() -> Self {
        let captured_at_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        Self {
            schema_version: 1,
            captured_at_unix_ms,
            devices: Vec::new(),
            warnings: Vec::new(),
            successful_collectors: BTreeSet::new(),
            failed_collectors: BTreeSet::new(),
        }
    }

    /// Overlay a fast/dynamic snapshot on a full/static snapshot.
    ///
    /// Stable labels from the base are retained when a dynamic collector only has
    /// a generic or unidentified label. This is useful when libsensors enrichment
    /// is intentionally performed only during the slower full collection pass.
    pub fn overlay(&self, dynamic: Snapshot) -> Snapshot {
        let mut merged = Snapshot::new();
        merged.schema_version = self.schema_version.max(dynamic.schema_version);
        merged.warnings = self.warnings.clone();
        merged.warnings.extend(dynamic.warnings);
        merged.warnings.sort();
        merged.warnings.dedup();
        merged.successful_collectors = dynamic.successful_collectors.clone();
        merged.failed_collectors = dynamic.failed_collectors.clone();

        let mut devices: BTreeMap<String, Device> = self
            .devices
            .iter()
            .cloned()
            .map(|device| (device.id.clone(), device))
            .collect();

        for mut incoming in dynamic.devices {
            match devices.get_mut(&incoming.id) {
                Some(existing) => {
                    if should_replace_name(&existing.name, &incoming.name) {
                        existing.name = std::mem::take(&mut incoming.name);
                    }
                    existing.merge_identity_from(&mut incoming);

                    let mut sensors = sensors_by_id(std::mem::take(&mut existing.sensors));
                    for mut sensor in incoming.sensors {
                        if let Some(previous) = sensors.get(&sensor.id) {
                            if sensor.identification == Identification::Unidentified
                                && previous.identification != Identification::Unidentified
                            {
                                sensor.label = previous.label.clone();
                                sensor.identification = previous.identification;
                            }
                        }
                        sensors.insert(sensor.id.clone(), sensor);
                    }
                    existing.sensors = sensors.into_values().collect();
                }
                None => {
                    devices.insert(incoming.id.clone(), incoming);
                }
            }
        }

        merged.devices = devices.into_values().collect();
        merged.sort();
        merged
    }

    pub(crate) fn mark_collector_succeeded(&mut self, collector: CollectorId) {
        self.failed_collectors.remove(&collector);
        self.successful_collectors.insert(collector);
    }

    pub(crate) fn collector_succeeded(&self, collector: CollectorId) -> bool {
        self.successful_collectors.contains(&collector)
    }

    pub(crate) fn collector_failed(&self, collector: CollectorId) -> bool {
        self.failed_collectors.contains(&collector)
    }

    pub fn sort(&mut self) {
        self.devices.sort_by(|a, b| {
            a.class
                .cmp(&b.class)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.id.cmp(&b.id))
        });
        for device in &mut self.devices {
            device.sensors.sort_by(|a, b| {
                a.kind
                    .cmp(&b.kind)
                    .then_with(|| natural_cmp(&a.label, &b.label))
                    .then_with(|| a.id.cmp(&b.id))
            });
        }
    }

    #[cfg(test)]
    pub(crate) fn mark_collector_failed(&mut self, collector: CollectorId) {
        self.successful_collectors.remove(&collector);
        self.failed_collectors.insert(collector);
    }
}

pub fn natural_cmp(left: &str, right: &str) -> Ordering {
    let left = left.to_lowercase();
    let right = right.to_lowercase();
    let left = left.as_bytes();
    let right = right.as_bytes();
    let (mut left_index, mut right_index) = (0, 0);

    while left_index < left.len() && right_index < right.len() {
        let left_digit = left[left_index].is_ascii_digit();
        let right_digit = right[right_index].is_ascii_digit();
        if left_digit && right_digit {
            let left_end = digit_run_end(left, left_index);
            let right_end = digit_run_end(right, right_index);
            let ordering =
                compare_digit_runs(&left[left_index..left_end], &right[right_index..right_end]);
            if ordering != Ordering::Equal {
                return ordering;
            }
            left_index = left_end;
            right_index = right_end;
            continue;
        }

        if left_digit != right_digit {
            return left[left_index].cmp(&right[right_index]);
        }

        let left_end = text_run_end(left, left_index);
        let right_end = text_run_end(right, right_index);
        let ordering = left[left_index..left_end].cmp(&right[right_index..right_end]);
        if ordering != Ordering::Equal {
            return ordering;
        }
        left_index = left_end;
        right_index = right_end;
    }

    left.len().cmp(&right.len())
}

fn digit_run_end(value: &[u8], start: usize) -> usize {
    value[start..]
        .iter()
        .position(|byte| !byte.is_ascii_digit())
        .map_or(value.len(), |offset| start + offset)
}

fn text_run_end(value: &[u8], start: usize) -> usize {
    value[start..]
        .iter()
        .position(u8::is_ascii_digit)
        .map_or(value.len(), |offset| start + offset)
}

fn compare_digit_runs(left: &[u8], right: &[u8]) -> Ordering {
    let left_significant = trim_leading_zeroes(left);
    let right_significant = trim_leading_zeroes(right);
    left_significant
        .len()
        .cmp(&right_significant.len())
        .then_with(|| left_significant.cmp(right_significant))
        .then_with(|| left.len().cmp(&right.len()))
}

fn trim_leading_zeroes(value: &[u8]) -> &[u8] {
    let first_nonzero = value
        .iter()
        .position(|byte| *byte != b'0')
        .unwrap_or(value.len().saturating_sub(1));
    &value[first_nonzero..]
}

#[derive(Debug, Default)]
pub(crate) struct SnapshotBuilder {
    devices: BTreeMap<String, Device>,
    warnings: Vec<String>,
    successful_collectors: BTreeSet<CollectorId>,
    failed_collectors: BTreeSet<CollectorId>,
}

impl SnapshotBuilder {
    pub(crate) fn add_device(&mut self, mut incoming: Device) {
        match self.devices.get_mut(&incoming.id) {
            Some(existing) => {
                let incoming_has_model = incoming.model.is_some();
                if should_replace_name(&existing.name, &incoming.name)
                    || (incoming_has_model && existing.model.is_none())
                {
                    existing.name = std::mem::take(&mut incoming.name);
                }
                replace_if_some(&mut existing.parent, &mut incoming.parent);
                existing.merge_identity_from(&mut incoming);
                if class_rank(&incoming.class) > class_rank(&existing.class) {
                    existing.class = incoming.class;
                }

                let mut sensors = sensors_by_id(std::mem::take(&mut existing.sensors));
                for sensor in incoming.sensors {
                    sensors.insert(sensor.id.clone(), sensor);
                }
                existing.sensors = sensors.into_values().collect();
            }
            None => {
                self.devices.insert(incoming.id.clone(), incoming);
            }
        }
    }

    pub(crate) fn warn(&mut self, warning: impl Into<String>) {
        self.warnings.push(warning.into());
    }

    pub(crate) fn mark_collector_succeeded(&mut self, collector: CollectorId) {
        self.failed_collectors.remove(&collector);
        self.successful_collectors.insert(collector);
    }

    pub(crate) fn mark_collector_failed(&mut self, collector: CollectorId) {
        self.successful_collectors.remove(&collector);
        self.failed_collectors.insert(collector);
    }

    pub(crate) fn finish(self) -> Snapshot {
        let mut snapshot = Snapshot::new();
        snapshot.devices = self.devices.into_values().collect();
        snapshot.warnings = self.warnings;
        snapshot.successful_collectors = self.successful_collectors;
        snapshot.failed_collectors = self.failed_collectors;
        for device in &mut snapshot.devices {
            for sensor in &mut device.sensors {
                if sensor.is_current() && sensor.last_updated_unix_ms.is_none() {
                    sensor.last_updated_unix_ms = Some(snapshot.captured_at_unix_ms);
                }
            }
        }
        snapshot.sort();
        snapshot
    }
}

fn replace_if_some<T>(target: &mut Option<T>, source: &mut Option<T>) {
    if let Some(value) = source.take() {
        *target = Some(value);
    }
}

fn sensors_by_id(sensors: Vec<Sensor>) -> BTreeMap<String, Sensor> {
    sensors
        .into_iter()
        .map(|sensor| (sensor.id.clone(), sensor))
        .collect()
}

fn class_rank(class: &DeviceClass) -> u8 {
    match class {
        DeviceClass::Other => 0,
        DeviceClass::Pci | DeviceClass::Usb => 1,
        DeviceClass::SensorController | DeviceClass::Thermal => 2,
        _ => 3,
    }
}

fn should_replace_name(existing: &str, incoming: &str) -> bool {
    if incoming.is_empty()
        || incoming == "CPU"
        || incoming.starts_with("GPU card")
        || incoming.ends_with(" sensor controller")
    {
        return false;
    }
    existing.is_empty()
        || existing.starts_with("Unknown")
        || existing.starts_with("PCI device ")
        || existing.starts_with("GPU card")
        || existing.starts_with("sysfs:")
        || existing == incoming
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_sort_orders_numbered_sensor_labels_numerically() {
        let mut labels = vec!["CPU 10", "CPU 2", "CPU 1", "CPU 02"];
        labels.sort_by(|left, right| natural_cmp(left, right));

        assert_eq!(labels, vec!["CPU 1", "CPU 2", "CPU 02", "CPU 10"]);
    }

    #[test]
    fn snapshot_sort_uses_natural_sensor_ordering() {
        let mut snapshot = Snapshot::new();
        let mut cpu = Device::new("cpu:0", DeviceClass::Cpu, "CPU");
        for number in [10, 2, 1] {
            cpu.sensors.push(Sensor::new(
                format!("cpu:{number}:utilization"),
                format!("CPU {number} utilization"),
                SensorKind::Utilization,
                Unit::Percent,
                Some(0.0),
                "/proc/stat",
                Identification::Inferred,
            ));
        }
        snapshot.devices.push(cpu);
        snapshot.sort();

        assert_eq!(
            snapshot.devices[0]
                .sensors
                .iter()
                .map(|sensor| sensor.label.as_str())
                .collect::<Vec<_>>(),
            vec![
                "CPU 1 utilization",
                "CPU 2 utilization",
                "CPU 10 utilization",
            ]
        );
    }

    #[test]
    fn overlay_keeps_enriched_label() {
        let mut base = Snapshot::new();
        let mut device = Device::new("cpu:0", DeviceClass::Cpu, "Example CPU");
        let mut enriched = Sensor::new(
            "cpu:0:temp1",
            "CPU Package",
            SensorKind::Temperature,
            Unit::Celsius,
            Some(40.0),
            "sensors -j",
            Identification::LibSensorsConfig,
        );
        enriched
            .metadata
            .insert("computed_by".to_owned(), "lm-sensors".into());
        device.sensors.push(enriched);
        base.devices.push(device);

        let mut dynamic = Snapshot::new();
        let mut dynamic_device = Device::new("cpu:0", DeviceClass::Cpu, "CPU");
        dynamic_device.sensors.push(Sensor::new(
            "cpu:0:temp1",
            "Temperature 1",
            SensorKind::Temperature,
            Unit::Celsius,
            Some(41.0),
            "/sys/class/hwmon/hwmon0/temp1_input",
            Identification::Unidentified,
        ));
        dynamic.devices.push(dynamic_device);

        let merged = base.overlay(dynamic);
        let sensor = &merged.devices[0].sensors[0];
        assert_eq!(sensor.label, "CPU Package");
        assert_eq!(sensor.value, Some(41.0));
        assert_eq!(sensor.identification, Identification::LibSensorsConfig);
        assert_eq!(sensor.status, SensorStatus::Ok);
        assert_eq!(sensor.freshness, ReadingFreshness::Current);
    }

    #[test]
    fn builder_merges_memory_inventory_and_slot_telemetry() {
        let mut builder = SnapshotBuilder::default();

        let mut inventory = Device::new(
            "memory:slot:dimm-a2",
            DeviceClass::Memory,
            "Kingston KF560C30",
        );
        inventory
            .properties
            .insert("memory_role".to_owned(), "module".into());
        inventory
            .properties
            .insert("inventory_source".to_owned(), "dmidecode".into());
        inventory
            .properties
            .insert("size".to_owned(), "32 GB".into());
        builder.add_device(inventory);

        let mut telemetry = Device::new(
            "memory:slot:dimm-a2",
            DeviceClass::Memory,
            "SPD5118 — DIMM_A2",
        );
        telemetry.driver = Some("spd5118".to_owned());
        telemetry.sensors.push(Sensor::new(
            "memory:slot:dimm-a2:temp1",
            "Module temperature",
            SensorKind::Temperature,
            Unit::Celsius,
            Some(48.8),
            "/sys/class/hwmon/hwmon0/temp1_input",
            Identification::KnownDriverMapping,
        ));
        builder.add_device(telemetry);

        let snapshot = builder.finish();
        assert_eq!(snapshot.devices.len(), 1);
        let device = &snapshot.devices[0];
        assert_eq!(device.name, "Kingston KF560C30");
        assert_eq!(device.sensors.len(), 1);
        assert!(device.properties.contains_key("inventory_source"));
    }

    #[test]
    fn builder_preserves_primary_driver_when_telemetry_has_its_own_provider() {
        let mut builder = SnapshotBuilder::default();

        let mut cpu = Device::new("cpu:0", DeviceClass::Cpu, "Example CPU");
        cpu.driver = Some("amd-pstate-epp".to_owned());
        builder.add_device(cpu);

        let mut telemetry = Device::new("cpu:0", DeviceClass::Cpu, "CPU sensor controller");
        telemetry
            .properties
            .insert("hwmon_driver".to_owned(), "k10temp".into());
        telemetry.sensors.push(Sensor::new(
            "cpu:0:temp1",
            "Tctl",
            SensorKind::Temperature,
            Unit::Celsius,
            Some(45.0),
            "/sys/class/hwmon/hwmon0/temp1_input",
            Identification::KernelLabel,
        ));
        builder.add_device(telemetry);

        let snapshot = builder.finish();
        let device = &snapshot.devices[0];
        assert_eq!(device.driver.as_deref(), Some("amd-pstate-epp"));
        assert_eq!(
            device.properties.get("hwmon_driver"),
            Some(&PropertyValue::String("k10temp".to_owned()))
        );
        assert_eq!(device.sensors.len(), 1);
    }
}
