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
    AlertDirection, AlertEngine, AlertEvent, AlertRule, AlertSeverity, AlertState,
    DEFAULT_CRITICAL_COLOR, DEFAULT_WARNING_COLOR, alert_direction, alert_supported_sensor,
    rule_summary, sensor_key, unit_suffix, valid_alert_color,
};
pub use hardware::{
    HardwareCategory, HardwareCategoryKind, HardwareDevice, HardwareInventory, HardwareProperty,
    HardwareSection, HardwareSensor, build_hardware_inventory, hardware_device_text,
    hardware_inventory_text, storage_health_availability_text,
};
pub use logging::{
    LogFileWriter, LogFormat, LogScope, LogWorker, default_log_directory, timestamped_log_path,
};
pub use plasma::{
    MAIN_WINDOW_TITLE, plasma_window_placement_supported, sync_plasma_window_placement,
};
pub use presentation::{SensorPresentation, present_sensor};
pub use rows::{
    RowKind, RowOptions, SensorOrderEntry, SensorRow, build_sensor_rows,
    build_sensor_rows_with_order, ordered_device_entries, sensor_order_entries,
};
pub use settings::{
    AppSettings, ColumnSettings, DEFAULT_HISTORY_RETENTION_SECONDS, Density,
    MAX_HISTORY_RETENTION_SECONDS, MIN_HISTORY_RETENTION_SECONDS, MIN_REFRESH_INTERVAL_MS,
    SettingsStore, ThemePreference,
};
pub use terminal::{TerminalView, render_terminal_view};
pub use visibility::VisibilityState;
