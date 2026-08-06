use hwall_core::{Sensor, SensorKind, SensorStatus, Snapshot, Unit};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

pub const DEFAULT_WARNING_COLOR: &str = "#e5a50a";
pub const DEFAULT_CRITICAL_COLOR: &str = "#c01c28";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum AlertSeverity {
    #[default]
    Normal,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AlertState {
    #[default]
    Normal,
    WarningPending,
    CriticalPending,
    Warning,
    Critical,
    Suspended,
}

impl AlertState {
    pub const fn severity(self) -> AlertSeverity {
        match self {
            Self::Normal => AlertSeverity::Normal,
            Self::WarningPending | Self::Warning => AlertSeverity::Warning,
            Self::CriticalPending | Self::Critical => AlertSeverity::Critical,
            Self::Suspended => AlertSeverity::Normal,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::WarningPending => "Warning pending",
            Self::CriticalPending => "Critical pending",
            Self::Warning => "Warning",
            Self::Critical => "Critical",
            Self::Suspended => "Suspended",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AlertRule {
    pub warning_below: Option<f64>,
    pub critical_below: Option<f64>,
    pub warning_above: Option<f64>,
    pub critical_above: Option<f64>,
    pub duration_seconds: u64,
    pub hysteresis: f64,
    pub cooldown_seconds: u64,
    pub desktop_notifications: bool,
    pub recovery_notifications: bool,
    pub warning_color: Option<String>,
    pub critical_color: Option<String>,
}

impl Default for AlertRule {
    fn default() -> Self {
        Self {
            warning_below: None,
            critical_below: None,
            warning_above: None,
            critical_above: None,
            duration_seconds: 10,
            hysteresis: 0.0,
            cooldown_seconds: 600,
            desktop_notifications: true,
            recovery_notifications: true,
            warning_color: None,
            critical_color: None,
        }
    }
}

impl AlertRule {
    pub fn is_configured(&self) -> bool {
        [
            self.warning_below,
            self.critical_below,
            self.warning_above,
            self.critical_above,
        ]
        .into_iter()
        .flatten()
        .any(f64::is_finite)
    }

    pub fn severity_for_value(&self, value: f64) -> AlertSeverity {
        if !value.is_finite() {
            return AlertSeverity::Normal;
        }
        if self
            .critical_below
            .is_some_and(|threshold| value <= threshold)
            || self
                .critical_above
                .is_some_and(|threshold| value >= threshold)
        {
            AlertSeverity::Critical
        } else if self
            .warning_below
            .is_some_and(|threshold| value <= threshold)
            || self
                .warning_above
                .is_some_and(|threshold| value >= threshold)
        {
            AlertSeverity::Warning
        } else {
            AlertSeverity::Normal
        }
    }

    fn retained_severity(&self, value: f64, active: AlertSeverity) -> AlertSeverity {
        match active {
            AlertSeverity::Critical => {
                if threshold_retained(
                    value,
                    self.critical_below,
                    self.critical_above,
                    self.hysteresis,
                ) {
                    AlertSeverity::Critical
                } else if threshold_retained(
                    value,
                    self.warning_below,
                    self.warning_above,
                    self.hysteresis,
                ) {
                    AlertSeverity::Warning
                } else {
                    AlertSeverity::Normal
                }
            }
            AlertSeverity::Warning => {
                if threshold_retained(
                    value,
                    self.warning_below,
                    self.warning_above,
                    self.hysteresis,
                ) {
                    AlertSeverity::Warning
                } else {
                    AlertSeverity::Normal
                }
            }
            AlertSeverity::Normal => AlertSeverity::Normal,
        }
    }

    pub fn warning_color(&self) -> &str {
        self.warning_color
            .as_deref()
            .filter(|color| valid_alert_color(color))
            .unwrap_or(DEFAULT_WARNING_COLOR)
    }

    pub fn critical_color(&self) -> &str {
        self.critical_color
            .as_deref()
            .filter(|color| valid_alert_color(color))
            .unwrap_or(DEFAULT_CRITICAL_COLOR)
    }

    pub fn color_for(&self, severity: AlertSeverity) -> Option<&str> {
        match severity {
            AlertSeverity::Normal => None,
            AlertSeverity::Warning => Some(self.warning_color()),
            AlertSeverity::Critical => Some(self.critical_color()),
        }
    }
}

fn threshold_retained(value: f64, below: Option<f64>, above: Option<f64>, hysteresis: f64) -> bool {
    let hysteresis = hysteresis.max(0.0);
    below.is_some_and(|threshold| value <= threshold + hysteresis)
        || above.is_some_and(|threshold| value >= threshold - hysteresis)
}

pub fn valid_alert_color(color: &str) -> bool {
    let bytes = color.as_bytes();
    bytes.len() == 7 && bytes[0] == b'#' && bytes[1..].iter().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlertEvent {
    pub sensor_key: String,
    pub device_name: String,
    pub sensor_name: String,
    pub severity: AlertSeverity,
    pub recovered: bool,
    pub notify: bool,
    pub value: String,
}

#[derive(Debug, Clone)]
struct Candidate {
    severity: AlertSeverity,
    since: Instant,
}

#[derive(Debug, Clone, Default)]
struct RuntimeAlert {
    active: AlertSeverity,
    candidate: Option<Candidate>,
    suspended: bool,
    last_notification: Option<(Instant, AlertSeverity)>,
    episode_notified: bool,
}

#[derive(Debug, Default)]
pub struct AlertEngine {
    entries: BTreeMap<String, RuntimeAlert>,
}

impl AlertEngine {
    pub fn state(&self, sensor_key: &str) -> AlertState {
        let Some(runtime) = self.entries.get(sensor_key) else {
            return AlertState::Normal;
        };
        if runtime.suspended {
            return AlertState::Suspended;
        }
        match runtime
            .candidate
            .as_ref()
            .map(|candidate| candidate.severity)
        {
            Some(AlertSeverity::Critical) => AlertState::CriticalPending,
            Some(AlertSeverity::Warning) if runtime.active == AlertSeverity::Normal => {
                AlertState::WarningPending
            }
            _ => match runtime.active {
                AlertSeverity::Normal => AlertState::Normal,
                AlertSeverity::Warning => AlertState::Warning,
                AlertSeverity::Critical => AlertState::Critical,
            },
        }
    }

    pub fn states(&self) -> BTreeMap<String, AlertState> {
        self.entries
            .keys()
            .map(|key| (key.clone(), self.state(key)))
            .collect()
    }

    pub fn reset(&mut self, sensor_key: &str) {
        self.entries.remove(sensor_key);
    }

    pub fn evaluate(
        &mut self,
        snapshot: &Snapshot,
        rules: &BTreeMap<String, AlertRule>,
        now: Instant,
    ) -> Vec<AlertEvent> {
        self.entries.retain(|key, _| rules.contains_key(key));
        let mut events = Vec::new();
        let mut seen = BTreeSet::new();

        for device in &snapshot.devices {
            for sensor in &device.sensors {
                let key = sensor_key(&device.id, &sensor.id);
                let Some(rule) = rules
                    .get(&key)
                    .filter(|rule| rule.is_configured() && alert_supported_sensor(sensor))
                else {
                    self.entries.remove(&key);
                    continue;
                };
                seen.insert(key.clone());
                if !sensor.is_current()
                    || matches!(
                        sensor.status,
                        SensorStatus::Fault | SensorStatus::Unavailable
                    )
                {
                    let runtime = self.entries.entry(key).or_default();
                    runtime.candidate = None;
                    runtime.suspended = true;
                    continue;
                }
                let Some(value) = sensor.value.filter(|value| value.is_finite()) else {
                    let runtime = self.entries.entry(key).or_default();
                    runtime.candidate = None;
                    runtime.suspended = true;
                    continue;
                };

                let runtime = self.entries.entry(key.clone()).or_default();
                runtime.suspended = false;
                let measured = rule.severity_for_value(value);
                let previous = runtime.active;

                if measured > runtime.active {
                    let candidate_matches = runtime
                        .candidate
                        .as_ref()
                        .is_some_and(|candidate| candidate.severity == measured);
                    if !candidate_matches {
                        runtime.candidate = Some(Candidate {
                            severity: measured,
                            since: now,
                        });
                    }
                    let duration = Duration::from_secs(rule.duration_seconds);
                    let ready = duration.is_zero()
                        || runtime.candidate.as_ref().is_some_and(|candidate| {
                            now.duration_since(candidate.since) >= duration
                        });
                    if ready {
                        runtime.active = measured;
                        runtime.candidate = None;
                    }
                } else if measured < runtime.active {
                    runtime.active = rule.retained_severity(value, runtime.active);
                    runtime.candidate = None;
                } else {
                    runtime.candidate = None;
                }

                if runtime.active != previous {
                    let deescalated = runtime.active < previous;
                    let notify = if deescalated {
                        rule.desktop_notifications
                            && rule.recovery_notifications
                            && runtime.episode_notified
                    } else {
                        rule.desktop_notifications
                            && notification_allowed(runtime, runtime.active, rule, now)
                    };
                    if runtime.active == AlertSeverity::Normal {
                        runtime.episode_notified = false;
                    } else if notify {
                        runtime.last_notification = Some((now, runtime.active));
                        runtime.episode_notified = true;
                    }
                    events.push(AlertEvent {
                        sensor_key: key,
                        device_name: device.name.clone(),
                        sensor_name: sensor.label.clone(),
                        severity: runtime.active,
                        recovered: deescalated,
                        notify,
                        value: hwall_core::render::format_value(value, &sensor.unit),
                    });
                }
            }
        }

        for (key, runtime) in &mut self.entries {
            if !seen.contains(key) {
                runtime.candidate = None;
                runtime.suspended = true;
            }
        }

        events
    }
}

fn notification_allowed(
    runtime: &RuntimeAlert,
    severity: AlertSeverity,
    rule: &AlertRule,
    now: Instant,
) -> bool {
    let Some((last, last_severity)) = runtime.last_notification else {
        return true;
    };
    severity > last_severity
        || now.duration_since(last) >= Duration::from_secs(rule.cooldown_seconds)
}

pub fn alert_supported_sensor(sensor: &Sensor) -> bool {
    !matches!(sensor.unit, Unit::Boolean | Unit::Raw) && !sensor.is_intermittent()
}

pub fn sensor_key(device_id: &str, sensor_id: &str) -> String {
    format!("sensor:{device_id}:{sensor_id}")
}

pub fn alert_direction(kind: SensorKind) -> AlertDirection {
    match kind {
        SensorKind::Fan => AlertDirection::Below,
        SensorKind::Temperature
        | SensorKind::Power
        | SensorKind::Energy
        | SensorKind::Throughput
        | SensorKind::Utilization => AlertDirection::Above,
        SensorKind::Voltage
        | SensorKind::Current
        | SensorKind::Frequency
        | SensorKind::EffectiveClock
        | SensorKind::Capacity
        | SensorKind::Humidity
        | SensorKind::Counter
        | SensorKind::Boolean
        | SensorKind::Other => AlertDirection::Both,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertDirection {
    Above,
    Below,
    Both,
}

pub const fn unit_suffix(unit: Unit) -> &'static str {
    match unit {
        Unit::Celsius => "°C",
        Unit::Volt => "V",
        Unit::Ampere => "A",
        Unit::Watt => "W",
        Unit::WattHour => "Wh",
        Unit::AmpereHour => "Ah",
        Unit::Joule => "J",
        Unit::Rpm => "RPM",
        Unit::Hertz => "Hz",
        Unit::Percent => "%",
        Unit::Byte => "B",
        Unit::BytePerSecond => "B/s",
        Unit::Count => "",
        Unit::CountPerSecond => "/s",
        Unit::Boolean => "",
        Unit::Raw => "",
    }
}

pub fn rule_summary(rule: &AlertRule, unit: Unit) -> String {
    let suffix = unit_suffix(unit);
    let mut parts = Vec::new();
    push_threshold(&mut parts, "Warning below", rule.warning_below, suffix);
    push_threshold(&mut parts, "Critical below", rule.critical_below, suffix);
    push_threshold(&mut parts, "Warning above", rule.warning_above, suffix);
    push_threshold(&mut parts, "Critical above", rule.critical_above, suffix);
    if parts.is_empty() {
        "Not configured".to_owned()
    } else {
        parts.join(" · ")
    }
}

fn push_threshold(parts: &mut Vec<String>, label: &str, value: Option<f64>, suffix: &str) {
    if let Some(value) = value {
        let separator = if suffix.is_empty() { "" } else { " " };
        parts.push(format!("{label} {value}{separator}{suffix}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwall_core::{Device, DeviceClass, Identification};

    fn snapshot(value: f64) -> Snapshot {
        let mut snapshot = Snapshot::new();
        let mut device = Device::new("cpu:0", DeviceClass::Cpu, "CPU");
        device.sensors.push(Sensor::new(
            "temp:package",
            "Package",
            SensorKind::Temperature,
            Unit::Celsius,
            Some(value),
            "/test",
            Identification::KernelLabel,
        ));
        snapshot.devices.push(device);
        snapshot
    }

    #[test]
    fn classifies_each_value_independently() {
        let rule = AlertRule {
            warning_above: Some(80.0),
            critical_above: Some(90.0),
            ..AlertRule::default()
        };
        assert_eq!(rule.severity_for_value(70.0), AlertSeverity::Normal);
        assert_eq!(rule.severity_for_value(85.0), AlertSeverity::Warning);
        assert_eq!(rule.severity_for_value(95.0), AlertSeverity::Critical);
    }

    #[test]
    fn duration_delays_active_transition() {
        let rule = AlertRule {
            warning_above: Some(80.0),
            duration_seconds: 10,
            desktop_notifications: false,
            ..AlertRule::default()
        };
        let key = sensor_key("cpu:0", "temp:package");
        let rules = BTreeMap::from([(key.clone(), rule)]);
        let start = Instant::now();
        let mut engine = AlertEngine::default();
        engine.evaluate(&snapshot(85.0), &rules, start);
        assert_eq!(engine.state(&key), AlertState::WarningPending);
        engine.evaluate(&snapshot(85.0), &rules, start + Duration::from_secs(10));
        assert_eq!(engine.state(&key), AlertState::Warning);
    }

    #[test]
    fn recovery_is_reported_without_using_statistics() {
        let rule = AlertRule {
            warning_above: Some(80.0),
            duration_seconds: 0,
            cooldown_seconds: 600,
            ..AlertRule::default()
        };
        let key = sensor_key("cpu:0", "temp:package");
        let rules = BTreeMap::from([(key.clone(), rule)]);
        let start = Instant::now();
        let mut engine = AlertEngine::default();
        let active = engine.evaluate(&snapshot(85.0), &rules, start);
        assert_eq!(active.len(), 1);
        assert!(!active[0].recovered);
        let recovered = engine.evaluate(&snapshot(70.0), &rules, start + Duration::from_secs(1));
        assert_eq!(recovered.len(), 1);
        assert!(recovered[0].recovered);
        assert_eq!(recovered[0].severity, AlertSeverity::Normal);
        assert_eq!(engine.state(&key), AlertState::Normal);
    }

    #[test]
    fn suppressed_reactivation_does_not_emit_a_recovery_notice() {
        let rule = AlertRule {
            warning_above: Some(80.0),
            duration_seconds: 0,
            cooldown_seconds: 60,
            ..AlertRule::default()
        };
        let key = sensor_key("cpu:0", "temp:package");
        let rules = BTreeMap::from([(key, rule)]);
        let start = Instant::now();
        let mut engine = AlertEngine::default();
        assert!(engine.evaluate(&snapshot(85.0), &rules, start)[0].notify);
        engine.evaluate(&snapshot(70.0), &rules, start + Duration::from_secs(1));

        let reactivated = engine.evaluate(&snapshot(85.0), &rules, start + Duration::from_secs(2));
        assert_eq!(reactivated.len(), 1);
        assert!(!reactivated[0].notify);
        let recovered = engine.evaluate(&snapshot(70.0), &rules, start + Duration::from_secs(3));
        assert_eq!(recovered.len(), 1);
        assert!(!recovered[0].notify);
    }

    #[test]
    fn critical_to_warning_is_a_recovery_transition() {
        let rule = AlertRule {
            warning_above: Some(80.0),
            critical_above: Some(90.0),
            duration_seconds: 0,
            recovery_notifications: false,
            ..AlertRule::default()
        };
        let key = sensor_key("cpu:0", "temp:package");
        let rules = BTreeMap::from([(key.clone(), rule)]);
        let start = Instant::now();
        let mut engine = AlertEngine::default();
        engine.evaluate(&snapshot(95.0), &rules, start);

        let events = engine.evaluate(&snapshot(85.0), &rules, start + Duration::from_secs(1));

        assert_eq!(engine.state(&key), AlertState::Warning);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].severity, AlertSeverity::Warning);
        assert!(events[0].recovered);
        assert!(!events[0].notify);
    }

    #[test]
    fn intermittent_readings_are_not_evaluated() {
        let rule = AlertRule {
            warning_above: Some(80.0),
            duration_seconds: 0,
            ..AlertRule::default()
        };
        let key = sensor_key("cpu:0", "temp:package");
        let rules = BTreeMap::from([(key.clone(), rule)]);
        let mut snapshot = snapshot(85.0);
        snapshot.devices[0].sensors[0]
            .metadata
            .insert("intermittent".to_owned(), true.into());
        let mut engine = AlertEngine::default();

        assert!(
            engine
                .evaluate(&snapshot, &rules, Instant::now())
                .is_empty()
        );
        assert_eq!(engine.state(&key), AlertState::Normal);
    }

    #[test]
    fn hysteresis_prevents_immediate_recovery() {
        let rule = AlertRule {
            warning_above: Some(80.0),
            duration_seconds: 0,
            hysteresis: 3.0,
            desktop_notifications: false,
            ..AlertRule::default()
        };
        let key = sensor_key("cpu:0", "temp:package");
        let rules = BTreeMap::from([(key.clone(), rule)]);
        let start = Instant::now();
        let mut engine = AlertEngine::default();
        engine.evaluate(&snapshot(85.0), &rules, start);
        assert_eq!(engine.state(&key), AlertState::Warning);
        engine.evaluate(&snapshot(79.0), &rules, start + Duration::from_secs(1));
        assert_eq!(engine.state(&key), AlertState::Warning);
        engine.evaluate(&snapshot(76.0), &rules, start + Duration::from_secs(2));
        assert_eq!(engine.state(&key), AlertState::Normal);
    }

    #[test]
    fn stale_reading_suspends_active_alert_until_fresh_data_returns() {
        let rule = AlertRule {
            warning_above: Some(80.0),
            duration_seconds: 0,
            ..AlertRule::default()
        };
        let key = sensor_key("cpu:0", "temp:package");
        let rules = BTreeMap::from([(key.clone(), rule)]);
        let start = Instant::now();
        let mut engine = AlertEngine::default();
        engine.evaluate(&snapshot(85.0), &rules, start);
        assert_eq!(engine.state(&key), AlertState::Warning);

        let mut stale = snapshot(85.0);
        stale.devices[0].sensors[0].freshness = hwall_core::ReadingFreshness::Stale;
        assert!(
            engine
                .evaluate(&stale, &rules, start + Duration::from_secs(1))
                .is_empty()
        );
        assert_eq!(engine.state(&key), AlertState::Suspended);

        let recovered = engine.evaluate(&snapshot(70.0), &rules, start + Duration::from_secs(2));
        assert_eq!(engine.state(&key), AlertState::Normal);
        assert_eq!(recovered.len(), 1);
        assert!(recovered[0].recovered);
    }

    #[test]
    fn missing_sensor_suspends_active_alert_without_recovery_event() {
        let rule = AlertRule {
            warning_above: Some(80.0),
            duration_seconds: 0,
            ..AlertRule::default()
        };
        let key = sensor_key("cpu:0", "temp:package");
        let rules = BTreeMap::from([(key.clone(), rule)]);
        let start = Instant::now();
        let mut engine = AlertEngine::default();
        engine.evaluate(&snapshot(85.0), &rules, start);

        let empty = Snapshot::new();
        assert!(
            engine
                .evaluate(&empty, &rules, start + Duration::from_secs(1))
                .is_empty()
        );
        assert_eq!(engine.state(&key), AlertState::Suspended);
    }
}
