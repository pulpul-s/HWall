use crate::{present_sensor, sensor_key, AlertRule, AlertState, VisibilityState};
use hwall_core::render::sensor_kind_name;
use hwall_core::{Device, DeviceClass, Sensor, SensorKind, Snapshot, SnapshotStatistics};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RowKind {
    Device,
    Header,
    Sensor,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SensorRow {
    pub id: String,
    pub device_id: String,
    pub sensor_id: Option<String>,
    pub hide_key: String,
    pub kind: RowKind,
    pub depth: u8,
    pub label: String,
    pub original_label: String,
    pub current: String,
    pub minimum: String,
    pub maximum: String,
    pub average: String,
    pub status: String,
    pub current_color: Option<String>,
    pub minimum_color: Option<String>,
    pub maximum_color: Option<String>,
    pub average_color: Option<String>,
    pub status_color: Option<String>,
    pub favorite: bool,
    pub collapsed: bool,
}

struct SectionRow {
    id: String,
    device_id: String,
    kind: RowKind,
    depth: u8,
    label: String,
    original_label: String,
    status: String,
    collapsed: bool,
}

impl SensorRow {
    fn section(section: SectionRow) -> Self {
        Self {
            hide_key: section.id.clone(),
            id: section.id,
            device_id: section.device_id,
            sensor_id: None,
            kind: section.kind,
            depth: section.depth,
            label: section.label,
            original_label: section.original_label,
            current: String::new(),
            minimum: String::new(),
            maximum: String::new(),
            average: String::new(),
            status: section.status,
            current_color: None,
            minimum_color: None,
            maximum_color: None,
            average_color: None,
            status_color: None,
            favorite: false,
            collapsed: section.collapsed,
        }
    }

    fn matches_query(&self, query: &str) -> bool {
        format!(
            "{} {} {} {}",
            self.label, self.original_label, self.current, self.status
        )
        .to_lowercase()
        .contains(query)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RowOptions<'a> {
    pub visibility: &'a VisibilityState,
    pub sensor_aliases: &'a BTreeMap<String, String>,
    pub device_order: &'a [String],
    pub show_sensor_groups: bool,
    pub query: &'a str,
    pub favorites_only: bool,
    pub alert_rules: &'a BTreeMap<String, AlertRule>,
    pub alert_states: &'a BTreeMap<String, AlertState>,
}

struct SensorRowContext<'a> {
    statistics: &'a SnapshotStatistics,
    visibility: &'a VisibilityState,
    sensor_aliases: &'a BTreeMap<String, String>,
    alert_rules: &'a BTreeMap<String, AlertRule>,
    alert_states: &'a BTreeMap<String, AlertState>,
}

pub fn build_sensor_rows(
    snapshot: &Snapshot,
    statistics: &SnapshotStatistics,
    options: RowOptions<'_>,
) -> Vec<SensorRow> {
    let RowOptions {
        visibility,
        sensor_aliases,
        device_order,
        show_sensor_groups,
        query,
        favorites_only,
        alert_rules,
        alert_states,
    } = options;
    let row_context = SensorRowContext {
        statistics,
        visibility,
        sensor_aliases,
        alert_rules,
        alert_states,
    };
    let query = query.trim().to_lowercase();
    let mut rows = Vec::new();

    for device in ordered_devices(snapshot, device_order) {
        if device.sensors.is_empty() {
            continue;
        }
        let device_key = format!("device:{}", device.id);
        let device_label = device_display_label(device);
        let device_matches = !query.is_empty() && device_label.to_lowercase().contains(&query);
        if visibility.is_hidden(&device_key) {
            continue;
        }

        let groups = grouped_sensors(device);
        let mut projected_groups = Vec::new();
        for (kind, sensors) in groups {
            let group_label = sensor_kind_name(kind);
            let group_matches = !query.is_empty() && group_label.to_lowercase().contains(&query);
            let group_key = format!("group:{}:{}", device.id, kind.as_str());
            if visibility.is_hidden(&group_key) {
                continue;
            }
            let mut projected = Vec::new();
            for sensor in sensors {
                let sensor_key = sensor_key(&device.id, &sensor.id);
                if visibility.is_hidden(&sensor_key) {
                    continue;
                }
                if favorites_only && !visibility.is_favorite(&sensor_key) {
                    continue;
                }
                let row = sensor_row(
                    device,
                    sensor,
                    &sensor_key,
                    if show_sensor_groups { 2 } else { 1 },
                    &row_context,
                );
                let matches = query.is_empty()
                    || row.matches_query(&query)
                    || device_matches
                    || group_matches;
                if matches {
                    projected.push(row);
                }
            }
            if !projected.is_empty() {
                projected_groups.push((kind, group_key, projected));
            }
        }

        if projected_groups.is_empty() {
            continue;
        }

        let device_collapsed = visibility.is_collapsed(&device_key);
        rows.push(SensorRow::section(SectionRow {
            id: device_key,
            device_id: device.id.clone(),
            kind: RowKind::Device,
            depth: 0,
            label: device_label,
            original_label: device.name.clone(),
            status: device.driver.clone().unwrap_or_default(),
            collapsed: device_collapsed,
        }));
        if device_collapsed {
            continue;
        }

        for (kind, group_key, projected) in projected_groups {
            if show_sensor_groups {
                let group_collapsed = visibility.is_collapsed(&group_key);
                let label = sensor_kind_name(kind).to_owned();
                rows.push(SensorRow::section(SectionRow {
                    id: group_key,
                    device_id: device.id.clone(),
                    kind: RowKind::Header,
                    depth: 1,
                    label: label.clone(),
                    original_label: label,
                    status: format!("{} sensors", projected.len()),
                    collapsed: group_collapsed,
                }));
                if group_collapsed {
                    continue;
                }
            }
            rows.extend(projected);
        }
    }
    rows
}

pub fn ordered_device_entries(
    snapshot: &Snapshot,
    device_order: &[String],
) -> Vec<(String, String)> {
    ordered_devices(snapshot, device_order)
        .into_iter()
        .filter(|device| !device.sensors.is_empty())
        .map(|device| (device.id.clone(), device_display_label(device)))
        .collect()
}

fn ordered_devices<'a>(snapshot: &'a Snapshot, device_order: &[String]) -> Vec<&'a Device> {
    let positions: HashMap<&str, usize> = device_order
        .iter()
        .enumerate()
        .map(|(index, id)| (id.as_str(), index))
        .collect();
    let fallback = positions.len();
    let mut devices: Vec<(usize, usize, &Device)> = snapshot
        .devices
        .iter()
        .enumerate()
        .map(|(original_index, device)| {
            (
                positions
                    .get(device.id.as_str())
                    .copied()
                    .unwrap_or(fallback + original_index),
                original_index,
                device,
            )
        })
        .collect();
    devices.sort_by_key(|(order, original, _)| (*order, *original));
    devices.into_iter().map(|(_, _, device)| device).collect()
}

fn grouped_sensors(device: &Device) -> BTreeMap<SensorKind, Vec<&Sensor>> {
    let mut groups: BTreeMap<SensorKind, Vec<&Sensor>> = BTreeMap::new();
    for sensor in &device.sensors {
        groups.entry(sensor.kind).or_default().push(sensor);
    }
    groups
}

fn sensor_row(
    device: &Device,
    sensor: &Sensor,
    sensor_key: &str,
    depth: u8,
    context: &SensorRowContext<'_>,
) -> SensorRow {
    let rule = context.alert_rules.get(sensor_key);
    let state = context
        .alert_states
        .get(sensor_key)
        .copied()
        .unwrap_or(AlertState::Normal);
    let presentation = present_sensor(
        sensor,
        context.statistics.get(&device.id, &sensor.id).copied(),
        rule,
        state,
    );

    SensorRow {
        id: sensor_key.to_owned(),
        device_id: device.id.clone(),
        sensor_id: Some(sensor.id.clone()),
        hide_key: sensor_key.to_owned(),
        kind: RowKind::Sensor,
        depth,
        label: context
            .sensor_aliases
            .get(sensor_key)
            .filter(|alias| !alias.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| sensor.label.clone()),
        original_label: sensor.label.clone(),
        current: presentation.current,
        minimum: presentation.minimum,
        maximum: presentation.maximum,
        average: presentation.average,
        status: presentation.status,
        current_color: presentation.current_color,
        minimum_color: presentation.minimum_color,
        maximum_color: presentation.maximum_color,
        average_color: presentation.average_color,
        status_color: presentation.status_color,
        favorite: context.visibility.is_favorite(sensor_key),
        collapsed: false,
    }
}

fn device_display_label(device: &Device) -> String {
    if device.class != DeviceClass::Memory {
        return device.name.clone();
    }

    let locator = device
        .property_str("locator")
        .filter(|value| !value.trim().is_empty());
    let address = device
        .property_str("i2c_address")
        .filter(|value| !value.trim().is_empty());
    let mut details = Vec::new();
    if let Some(locator) = locator.filter(|value| !device.name.contains(value)) {
        details.push(locator.to_owned());
    }
    if let Some(address) = address {
        details.push(format!("I²C {address}"));
    }
    if details.is_empty() {
        device.name.clone()
    } else {
        format!("{} — {}", device.name, details.join(" · "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwall_core::{DeviceClass, Identification, Unit};

    fn snapshot() -> Snapshot {
        let mut cpu = Device::new("cpu:0", DeviceClass::Cpu, "Processor");
        cpu.sensors.push(Sensor::new(
            "temp:0",
            "CPU temperature",
            SensorKind::Temperature,
            Unit::Celsius,
            Some(42.0),
            "/test/cpu",
            Identification::KernelLabel,
        ));
        let mut gpu = Device::new("gpu:0", DeviceClass::Gpu, "Graphics card");
        gpu.sensors.push(Sensor::new(
            "fan:0",
            "Fan",
            SensorKind::Fan,
            Unit::Rpm,
            Some(800.0),
            "/test/gpu",
            Identification::KernelLabel,
        ));
        let mut snapshot = Snapshot::new();
        snapshot.devices = vec![cpu, gpu];
        snapshot
    }

    #[test]
    fn custom_device_order_is_applied() {
        let rows = build_sensor_rows(
            &snapshot(),
            &SnapshotStatistics::default(),
            RowOptions {
                visibility: &VisibilityState::default(),
                sensor_aliases: &BTreeMap::new(),
                device_order: &["gpu:0".to_owned(), "cpu:0".to_owned()],
                show_sensor_groups: false,
                query: "",
                favorites_only: false,
                alert_rules: &BTreeMap::new(),
                alert_states: &BTreeMap::new(),
            },
        );
        let devices: Vec<_> = rows
            .iter()
            .filter(|row| row.kind == RowKind::Device)
            .map(|row| row.label.as_str())
            .collect();
        assert_eq!(devices, vec!["Graphics card", "Processor"]);
    }

    #[test]
    fn groups_are_flattened_by_default() {
        let rows = build_sensor_rows(
            &snapshot(),
            &SnapshotStatistics::default(),
            RowOptions {
                visibility: &VisibilityState::default(),
                sensor_aliases: &BTreeMap::new(),
                device_order: &[],
                show_sensor_groups: false,
                query: "",
                favorites_only: false,
                alert_rules: &BTreeMap::new(),
                alert_states: &BTreeMap::new(),
            },
        );
        assert!(!rows.iter().any(|row| row.kind == RowKind::Header));
        assert!(rows
            .iter()
            .filter(|row| row.kind == RowKind::Sensor)
            .all(|row| row.depth == 1));
    }

    #[test]
    fn sensor_aliases_replace_the_display_label_but_keep_the_original() {
        let aliases =
            BTreeMap::from([("sensor:cpu:0:temp:0".to_owned(), "CPU package".to_owned())]);
        let rows = build_sensor_rows(
            &snapshot(),
            &SnapshotStatistics::default(),
            RowOptions {
                visibility: &VisibilityState::default(),
                sensor_aliases: &aliases,
                device_order: &[],
                show_sensor_groups: false,
                query: "",
                favorites_only: false,
                alert_rules: &BTreeMap::new(),
                alert_states: &BTreeMap::new(),
            },
        );
        let sensor = rows
            .iter()
            .find(|row| row.kind == RowKind::Sensor && row.id.contains("temp:0"))
            .expect("CPU sensor row");
        assert_eq!(sensor.label, "CPU package");
        assert_eq!(sensor.original_label, "CPU temperature");
    }

    #[test]
    fn memory_device_header_includes_i2c_address() {
        let mut memory = Device::new(
            "memory:spd5118:6-0053",
            DeviceClass::Memory,
            "SPD5118 DDR5 memory module",
        );
        memory
            .properties
            .insert("i2c_address".to_owned(), "0x53".into());
        memory.sensors.push(Sensor::new(
            "temp:0",
            "Module temperature",
            SensorKind::Temperature,
            Unit::Celsius,
            Some(48.0),
            "/test/memory",
            Identification::KnownDriverMapping,
        ));
        let mut snapshot = Snapshot::new();
        snapshot.devices.push(memory);
        let rows = build_sensor_rows(
            &snapshot,
            &SnapshotStatistics::default(),
            RowOptions {
                visibility: &VisibilityState::default(),
                sensor_aliases: &BTreeMap::new(),
                device_order: &[],
                show_sensor_groups: false,
                query: "",
                favorites_only: false,
                alert_rules: &BTreeMap::new(),
                alert_states: &BTreeMap::new(),
            },
        );
        assert_eq!(
            rows.first().map(|row| row.label.as_str()),
            Some("SPD5118 DDR5 memory module — I²C 0x53"),
        );
    }

    #[test]
    fn search_can_match_across_display_fields() {
        let rows = build_sensor_rows(
            &snapshot(),
            &SnapshotStatistics::default(),
            RowOptions {
                visibility: &VisibilityState::default(),
                sensor_aliases: &BTreeMap::new(),
                device_order: &[],
                show_sensor_groups: false,
                query: "temperature 42",
                favorites_only: false,
                alert_rules: &BTreeMap::new(),
                alert_states: &BTreeMap::new(),
            },
        );
        assert!(rows
            .iter()
            .any(|row| row.kind == RowKind::Sensor && row.id.contains("temp:0")));
    }

    #[test]
    fn alert_colors_are_applied_to_each_numeric_column() {
        let mut current = snapshot();
        current.devices[0].sensors[0].value = Some(95.0);
        let mut statistics = SnapshotStatistics::new();
        for value in [55.0, 99.0, 95.0] {
            let mut sample = snapshot();
            sample.devices[0].sensors[0].value = Some(value);
            statistics.observe(&sample);
        }
        let key = "sensor:cpu:0:temp:0".to_owned();
        let rules = BTreeMap::from([(
            key.clone(),
            AlertRule {
                warning_above: Some(80.0),
                critical_above: Some(90.0),
                ..AlertRule::default()
            },
        )]);
        let states = BTreeMap::from([(key, AlertState::Critical)]);
        let rows = build_sensor_rows(
            &current,
            &statistics,
            RowOptions {
                visibility: &VisibilityState::default(),
                sensor_aliases: &BTreeMap::new(),
                device_order: &[],
                show_sensor_groups: false,
                query: "",
                favorites_only: false,
                alert_rules: &rules,
                alert_states: &states,
            },
        );
        let sensor = rows
            .iter()
            .find(|row| row.kind == RowKind::Sensor && row.id.contains("temp:0"))
            .expect("CPU temperature row");
        assert_eq!(
            sensor.current_color.as_deref(),
            Some(crate::DEFAULT_CRITICAL_COLOR)
        );
        assert_eq!(sensor.minimum_color, None);
        assert_eq!(
            sensor.maximum_color.as_deref(),
            Some(crate::DEFAULT_CRITICAL_COLOR)
        );
        assert_eq!(
            sensor.average_color.as_deref(),
            Some(crate::DEFAULT_WARNING_COLOR)
        );
        assert_eq!(sensor.status, "Critical");
    }

    #[test]
    fn groups_can_be_enabled() {
        let rows = build_sensor_rows(
            &snapshot(),
            &SnapshotStatistics::default(),
            RowOptions {
                visibility: &VisibilityState::default(),
                sensor_aliases: &BTreeMap::new(),
                device_order: &[],
                show_sensor_groups: true,
                query: "",
                favorites_only: false,
                alert_rules: &BTreeMap::new(),
                alert_states: &BTreeMap::new(),
            },
        );
        assert!(rows.iter().any(|row| row.kind == RowKind::Header));
        assert!(rows
            .iter()
            .filter(|row| row.kind == RowKind::Sensor)
            .all(|row| row.depth == 2));
    }
}
