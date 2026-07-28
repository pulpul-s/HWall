//! GTK-independent application policy for HWall.
//!
//! This crate owns settings, visibility rules, presentation models, alerts,
//! logging, and desktop integration so the GTK crate can stay focused on
//! widgets and application lifecycle.

use directories::ProjectDirs;

pub const APPLICATION_ID: &str = "io.github.hwall.HWall";

fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("io.github", "hwall", "HWall")
}

mod alerts;
mod hardware;
mod logging;
mod plasma;
mod presentation;
mod rows;
mod settings;
mod terminal;
mod visibility;

pub use alerts::{
    alert_direction, alert_supported_sensor, rule_summary, sensor_key, unit_suffix,
    valid_alert_color, AlertDirection, AlertEngine, AlertEvent, AlertRule, AlertSeverity,
    AlertState, DEFAULT_CRITICAL_COLOR, DEFAULT_WARNING_COLOR,
};
pub use hardware::{
    build_hardware_inventory, storage_health_availability_text, HardwareCategory,
    HardwareCategoryKind, HardwareDevice, HardwareInventory, HardwareProperty, HardwareSection,
    HardwareSensor,
};
pub use logging::{
    default_log_directory, timestamped_log_path, LogFileWriter, LogFormat, LogScope, LogWorker,
};
pub use plasma::{
    plasma_window_placement_supported, sync_plasma_window_placement, MAIN_WINDOW_TITLE,
};
pub use presentation::{present_sensor, SensorPresentation};
pub use rows::{build_sensor_rows, ordered_device_entries, RowKind, RowOptions, SensorRow};
pub use settings::{
    AppSettings, ColumnSettings, Density, SettingsStore, DEFAULT_HISTORY_RETENTION_SECONDS,
    MAX_HISTORY_RETENTION_SECONDS, MIN_HISTORY_RETENTION_SECONDS,
};
pub use terminal::{render_terminal_view, TerminalView};
pub use visibility::VisibilityState;
