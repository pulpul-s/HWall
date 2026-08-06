use crate::{AlertRule, LogFormat, LogScope, VisibilityState, project_dirs};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

pub const MIN_REFRESH_INTERVAL_MS: u64 = 200;
pub const MIN_HISTORY_RETENTION_SECONDS: u64 = 60;
pub const MAX_HISTORY_RETENTION_SECONDS: u64 = 24 * 60 * 60;
pub const DEFAULT_HISTORY_RETENTION_SECONDS: u64 = MIN_HISTORY_RETENTION_SECONDS;

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThemePreference {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemePreference {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    pub const fn id(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    pub fn from_id(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|theme| theme.id() == value)
    }
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Density {
    #[default]
    Compact,
    Normal,
    Comfortable,
}

impl Density {
    pub const ALL: [Self; 3] = [Self::Compact, Self::Normal, Self::Comfortable];

    pub const fn id(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Normal => "normal",
            Self::Comfortable => "comfortable",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Compact => "Compact",
            Self::Normal => "Normal",
            Self::Comfortable => "Comfortable",
        }
    }

    pub fn from_id(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|density| density.id() == value)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ColumnSettings {
    pub id: String,
    pub width: i32,
    pub visible: bool,
}

impl ColumnSettings {
    pub fn new(id: impl Into<String>, width: i32, visible: bool) -> Self {
        Self {
            id: id.into(),
            width,
            visible,
        }
    }

    pub fn default_layout() -> Vec<Self> {
        vec![
            Self::new("sensor", 420, true),
            Self::new("current", 118, true),
            Self::new("minimum", 118, true),
            Self::new("maximum", 118, true),
            Self::new("average", 118, true),
            Self::new("status", 112, false),
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub window_width: i32,
    pub window_height: i32,
    pub maximized: bool,
    pub interval_ms: u64,
    pub rediscover_seconds: u64,
    pub health_interval_seconds: u64,
    pub density: Density,
    pub theme: ThemePreference,
    pub close_to_tray: bool,
    pub start_hidden: bool,
    pub plasma_window_placement: bool,
    pub favorites_only: bool,
    pub show_sensor_groups: bool,
    pub show_identifying_information: bool,
    pub history_retention_seconds: u64,
    pub device_order: Vec<String>,
    pub sensor_row_order: Vec<String>,
    pub device_aliases: BTreeMap<String, String>,
    pub sensor_aliases: BTreeMap<String, String>,
    pub sensor_alerts: BTreeMap<String, AlertRule>,
    pub columns: Vec<ColumnSettings>,
    pub visibility: VisibilityState,
    pub logging_format: LogFormat,
    pub logging_scope: LogScope,
    pub logging_directory: Option<PathBuf>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            window_width: 1040,
            window_height: 720,
            maximized: false,
            interval_ms: 1_000,
            rediscover_seconds: 30,
            health_interval_seconds: 1_800,
            density: Density::Compact,
            theme: ThemePreference::System,
            close_to_tray: false,
            start_hidden: false,
            plasma_window_placement: false,
            favorites_only: false,
            show_sensor_groups: false,
            show_identifying_information: false,
            history_retention_seconds: DEFAULT_HISTORY_RETENTION_SECONDS,
            device_order: Vec::new(),
            sensor_row_order: Vec::new(),
            device_aliases: BTreeMap::new(),
            sensor_aliases: BTreeMap::new(),
            sensor_alerts: BTreeMap::new(),
            columns: ColumnSettings::default_layout(),
            visibility: VisibilityState::default(),
            logging_format: LogFormat::Csv,
            logging_scope: LogScope::Visible,
            logging_directory: None,
        }
    }
}

impl AppSettings {
    pub fn refresh_interval(&self) -> Duration {
        Duration::from_millis(self.interval_ms.max(MIN_REFRESH_INTERVAL_MS))
    }

    pub fn history_retention(&self) -> Duration {
        Duration::from_secs(
            self.history_retention_seconds
                .clamp(MIN_HISTORY_RETENTION_SECONDS, MAX_HISTORY_RETENTION_SECONDS),
        )
    }
}

#[derive(Debug)]
pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    pub fn discover() -> Self {
        let path = project_dirs()
            .map(|dirs| dirs.config_dir().join("settings.json"))
            .unwrap_or_else(|| PathBuf::from("hwall-settings.json"));
        Self { path }
    }

    #[cfg(test)]
    fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn load(&self) -> AppSettings {
        let Ok(bytes) = fs::read(&self.path) else {
            return AppSettings::default();
        };
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    pub fn save(&self, settings: &AppSettings) -> io::Result<()> {
        let Some(parent) = self.path.parent() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "settings path has no parent",
            ));
        };
        fs::create_dir_all(parent)?;
        let temporary = self.path.with_extension("json.tmp");
        let serialized = serde_json::to_vec_pretty(settings)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        fs::write(&temporary, serialized)?;
        fs::rename(temporary, &self.path)
    }

    pub fn reset(&self) -> io::Result<()> {
        for path in [self.path.with_extension("json.tmp"), self.path.clone()] {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_ids_round_trip() {
        for theme in ThemePreference::ALL {
            assert_eq!(ThemePreference::from_id(theme.id()), Some(theme));
        }
        assert_eq!(ThemePreference::from_id("unknown"), None);
    }

    #[test]
    fn density_ids_round_trip() {
        for density in Density::ALL {
            assert_eq!(Density::from_id(density.id()), Some(density));
        }
        assert_eq!(Density::from_id("unknown"), None);
    }

    #[test]
    fn status_column_is_hidden_by_default() {
        let layout = ColumnSettings::default_layout();
        let status = layout
            .iter()
            .find(|column| column.id == "status")
            .expect("status column");
        assert!(!status.visible);
    }

    #[test]
    fn refresh_interval_respects_the_supported_minimum() {
        let settings = AppSettings {
            interval_ms: 100,
            ..AppSettings::default()
        };

        assert_eq!(
            settings.refresh_interval(),
            Duration::from_millis(MIN_REFRESH_INTERVAL_MS)
        );
    }

    #[test]
    fn settings_without_theme_use_system_default() {
        let path = std::env::temp_dir().join(format!(
            "hwall-settings-theme-default-test-{}-{}.json",
            std::process::id(),
            std::thread::current().name().unwrap_or("main")
        ));
        fs::write(&path, r#"{"interval_ms":750}"#).expect("write old settings");

        let loaded = SettingsStore::at(&path).load();
        assert_eq!(loaded.interval_ms, 750);
        assert_eq!(loaded.theme, ThemePreference::System);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn settings_round_trip() {
        let path = std::env::temp_dir().join(format!(
            "hwall-settings-test-{}-{}.json",
            std::process::id(),
            std::thread::current().name().unwrap_or("main")
        ));
        let store = SettingsStore::at(&path);
        let mut settings = AppSettings {
            interval_ms: 750,
            theme: ThemePreference::Dark,
            plasma_window_placement: true,
            show_identifying_information: true,
            history_retention_seconds: 3_600,
            device_order: vec!["gpu:0".to_owned(), "cpu:0".to_owned()],
            sensor_row_order: vec![
                "group:cpu:0:temperature".to_owned(),
                "sensor:cpu:0:temp:0".to_owned(),
            ],
            ..AppSettings::default()
        };
        settings.sensor_aliases.insert(
            "sensor:cpu:temp".to_owned(),
            "Package temperature".to_owned(),
        );
        settings
            .device_aliases
            .insert("cpu:0".to_owned(), "Main processor".to_owned());
        settings.sensor_alerts.insert(
            "sensor:cpu:temp".to_owned(),
            AlertRule {
                warning_above: Some(80.0),
                critical_above: Some(90.0),
                warning_color: Some("#ffaa00".to_owned()),
                ..AlertRule::default()
            },
        );
        settings
            .visibility
            .hide("sensor:cpu:temp", "CPU temperature");
        store.save(&settings).expect("save settings");
        let loaded = store.load();
        assert_eq!(loaded.interval_ms, 750);
        assert_eq!(loaded.theme, ThemePreference::Dark);
        assert!(loaded.plasma_window_placement);
        assert!(loaded.show_identifying_information);
        assert_eq!(loaded.history_retention(), Duration::from_secs(3_600));
        assert_eq!(loaded.device_order, vec!["gpu:0", "cpu:0"]);
        assert_eq!(
            loaded.sensor_row_order,
            vec!["group:cpu:0:temperature", "sensor:cpu:0:temp:0"]
        );
        assert_eq!(
            loaded.device_aliases.get("cpu:0").map(String::as_str),
            Some("Main processor"),
        );
        assert_eq!(
            loaded
                .sensor_aliases
                .get("sensor:cpu:temp")
                .map(String::as_str),
            Some("Package temperature"),
        );
        assert_eq!(
            loaded
                .sensor_alerts
                .get("sensor:cpu:temp")
                .and_then(|rule| rule.critical_above),
            Some(90.0),
        );
        assert!(loaded.visibility.is_hidden("sensor:cpu:temp"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn reset_removes_saved_and_temporary_settings() {
        let path = std::env::temp_dir().join(format!(
            "hwall-settings-reset-test-{}-{}.json",
            std::process::id(),
            std::thread::current().name().unwrap_or("main")
        ));
        let temporary = path.with_extension("json.tmp");
        let store = SettingsStore::at(&path);
        store.save(&AppSettings::default()).expect("save settings");
        fs::write(&temporary, b"partial").expect("write temporary settings");

        store.reset().expect("reset settings");

        assert!(!path.exists());
        assert!(!temporary.exists());
        store.reset().expect("reset missing settings");
    }
}
