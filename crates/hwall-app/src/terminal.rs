use crate::{
    build_hardware_inventory, build_sensor_rows, HardwareDevice, HardwareInventory, HardwareSensor,
    RowKind, RowOptions, SensorRow, VisibilityState,
};
use hwall_core::{Snapshot, SnapshotStatistics};
use std::collections::BTreeMap;
use std::fmt::Write;

const RULE_WIDTH: usize = 84;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalView {
    Mixed,
    Sensors,
    Hardware,
}

impl TerminalView {
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Mixed => "Mixed",
            Self::Sensors => "Sensors",
            Self::Hardware => "Hardware",
        }
    }

    pub const fn next(self) -> Self {
        match self {
            Self::Mixed => Self::Sensors,
            Self::Sensors => Self::Hardware,
            Self::Hardware => Self::Mixed,
        }
    }

    pub const fn previous(self) -> Self {
        match self {
            Self::Mixed => Self::Hardware,
            Self::Sensors => Self::Mixed,
            Self::Hardware => Self::Sensors,
        }
    }
}

pub fn render_terminal_view(
    snapshot: &Snapshot,
    statistics: &SnapshotStatistics,
    view: TerminalView,
) -> String {
    let aliases = BTreeMap::new();
    let rules = BTreeMap::new();
    let states = BTreeMap::new();

    let mut out = match view {
        TerminalView::Mixed | TerminalView::Hardware => {
            let inventory =
                build_hardware_inventory(snapshot, statistics, &aliases, &rules, &states);
            render_inventory(&inventory, view == TerminalView::Mixed)
        }
        TerminalView::Sensors => {
            let visibility = VisibilityState::default();
            let rows = build_sensor_rows(
                snapshot,
                statistics,
                RowOptions {
                    visibility: &visibility,
                    sensor_aliases: &aliases,
                    device_order: &[],
                    show_sensor_groups: true,
                    query: "",
                    favorites_only: false,
                    alert_rules: &rules,
                    alert_states: &states,
                },
            );
            render_sensor_rows(&rows)
        }
    };
    append_warnings(&mut out, &snapshot.warnings);
    out
}

fn render_inventory(inventory: &HardwareInventory, include_sensors: bool) -> String {
    let mut out = String::new();
    report_title(&mut out, "HWall hardware report");

    for category in &inventory.categories {
        section(&mut out, &category.label);
        for device in &category.devices {
            render_device(&mut out, device, include_sensors);
        }
    }
    out
}

fn render_device(out: &mut String, device: &HardwareDevice, include_sensors: bool) {
    let _ = writeln!(out, "{}", device.name);
    if !device.subtitle.trim().is_empty() {
        let _ = writeln!(out, "  {}", device.subtitle);
    }
    let _ = writeln!(out, "{}", "·".repeat(RULE_WIDTH));

    for section in &device.sections {
        subsection(out, &section.title);
        for item in &section.properties {
            property(out, &item.label, &item.value);
        }
    }
    if !device.advanced.is_empty() {
        subsection(out, "Advanced");
        for item in &device.advanced {
            property(out, &item.label, &item.value);
        }
    }
    if include_sensors && !device.sensors.is_empty() {
        render_hardware_sensors(out, &device.sensors);
    }
    let _ = writeln!(out);
}

fn render_hardware_sensors(out: &mut String, sensors: &[HardwareSensor]) {
    let observed = sensors
        .iter()
        .any(|sensor| has_observed_values(&sensor.minimum, &sensor.maximum, &sensor.average));
    let mut group = "";
    for sensor in sensors {
        if sensor.group != group {
            group = &sensor.group;
            subsection(out, group);
            sensor_header(out, observed);
        }
        sensor_line(
            out,
            SensorColumns {
                label: &sensor.label,
                current: &sensor.current,
                minimum: &sensor.minimum,
                maximum: &sensor.maximum,
                average: &sensor.average,
                status: &sensor.status,
            },
            observed,
        );
    }
}

fn render_sensor_rows(rows: &[SensorRow]) -> String {
    let mut out = String::new();
    report_title(&mut out, "HWall sensor report");
    let observed = rows.iter().any(|row| {
        row.kind == RowKind::Sensor && has_observed_values(&row.minimum, &row.maximum, &row.average)
    });

    for row in rows {
        match row.kind {
            RowKind::Device => {
                if out.ends_with("\n") && !out.ends_with("\n\n") {
                    let _ = writeln!(out);
                }
                let _ = writeln!(out, "{}", row.label);
                if !row.status.trim().is_empty() {
                    let _ = writeln!(out, "  {}", row.status);
                }
                let _ = writeln!(out, "{}", "·".repeat(RULE_WIDTH));
            }
            RowKind::Header => {
                subsection(&mut out, &row.label);
                sensor_header(&mut out, observed);
            }
            RowKind::Sensor => sensor_line(
                &mut out,
                SensorColumns {
                    label: &row.label,
                    current: &row.current,
                    minimum: &row.minimum,
                    maximum: &row.maximum,
                    average: &row.average,
                    status: &row.status,
                },
                observed,
            ),
        }
    }
    out
}

fn has_observed_values(minimum: &str, maximum: &str, average: &str) -> bool {
    [minimum, maximum, average]
        .into_iter()
        .any(|value| !value.trim().is_empty() && value != "—")
}

fn sensor_header(out: &mut String, observed: bool) {
    if observed {
        let _ = writeln!(
            out,
            "    {:<27} {:>11} {:>11} {:>11} {:>11}  Status",
            "Reading", "Current", "Minimum", "Maximum", "Average"
        );
    } else {
        let _ = writeln!(out, "    {:<47} {:>18}  Status", "Reading", "Current");
    }
}

struct SensorColumns<'a> {
    label: &'a str,
    current: &'a str,
    minimum: &'a str,
    maximum: &'a str,
    average: &'a str,
    status: &'a str,
}

fn sensor_line(out: &mut String, columns: SensorColumns<'_>, observed: bool) {
    if observed {
        let _ = writeln!(
            out,
            "    {:<27} {:>11} {:>11} {:>11} {:>11}  {}",
            truncate(columns.label, 27),
            truncate(columns.current, 11),
            truncate(columns.minimum, 11),
            truncate(columns.maximum, 11),
            truncate(columns.average, 11),
            columns.status,
        );
    } else {
        let _ = writeln!(
            out,
            "    {:<47} {:>18}  {}",
            truncate(columns.label, 47),
            truncate(columns.current, 18),
            columns.status,
        );
    }
}

fn append_warnings(out: &mut String, warnings: &[String]) {
    if warnings.is_empty() {
        return;
    }
    section(out, "Warnings");
    for warning in warnings {
        let _ = writeln!(out, "- {warning}");
    }
}

fn report_title(out: &mut String, title: &str) {
    ruled_heading(out, title, '=');
}

fn section(out: &mut String, title: &str) {
    ruled_heading(out, title, '-');
}

fn ruled_heading(out: &mut String, title: &str, rule: char) {
    let _ = writeln!(out, "{title}");
    let _ = writeln!(out, "{}", rule.to_string().repeat(RULE_WIDTH));
}

fn subsection(out: &mut String, title: &str) {
    let _ = writeln!(out, "  {title}");
}

fn property(out: &mut String, label: &str, value: &str) {
    if !value.trim().is_empty() {
        let _ = writeln!(out, "    {label:<30} {value}");
    }
}

fn truncate(value: &str, maximum: usize) -> String {
    let mut characters = value.chars();
    let prefix: String = characters.by_ref().take(maximum).collect();
    if characters.next().is_none() {
        prefix
    } else if maximum > 1 {
        prefix
            .chars()
            .take(maximum - 1)
            .chain(std::iter::once('…'))
            .collect()
    } else {
        "…".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwall_core::{Device, DeviceClass, Identification, Sensor, SensorKind, Unit};

    fn sample_snapshot() -> Snapshot {
        let mut board = Device::new("board:0", DeviceClass::Motherboard, "Example board");
        board
            .properties
            .insert("chassis_type".to_owned(), "3".into());
        let mut firmware = Device::new("bios:0", DeviceClass::Bios, "Firmware");
        firmware.parent = Some(board.id.clone());
        firmware.model = Some("1.2.3".to_owned());

        let mut cpu = Device::new("cpu:0", DeviceClass::Cpu, "Example CPU");
        cpu.sensors.push(Sensor::new(
            "temperature",
            "Package temperature",
            SensorKind::Temperature,
            Unit::Celsius,
            Some(42.0),
            "/test",
            Identification::KernelLabel,
        ));

        let mut snapshot = Snapshot::new();
        snapshot.devices = vec![board, firmware, cpu];
        snapshot
    }

    #[test]
    fn mixed_report_uses_the_shared_hardware_projection() {
        let snapshot = sample_snapshot();
        let report = render_terminal_view(
            &snapshot,
            &SnapshotStatistics::default(),
            TerminalView::Mixed,
        );
        assert!(report.contains("Motherboard & firmware"));
        assert!(report.contains("BIOS version"));
        assert!(report.contains("Desktop"));
        assert!(report.contains("Package temperature"));
        assert!(!report.contains("\nFirmware\n··"));
    }

    #[test]
    fn hardware_and_sensor_views_are_distinct() {
        let snapshot = sample_snapshot();
        let statistics = SnapshotStatistics::default();
        let hardware = render_terminal_view(&snapshot, &statistics, TerminalView::Hardware);
        let sensors = render_terminal_view(&snapshot, &statistics, TerminalView::Sensors);
        assert!(!hardware.contains("Package temperature"));
        assert!(sensors.contains("Package temperature"));
        assert!(!sensors.contains("BIOS version"));
    }
}
