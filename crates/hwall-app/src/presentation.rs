use crate::{AlertRule, AlertState, alert_supported_sensor};
use hwall_core::render::{format_reading_age_compact, format_sample_age_compact, format_value};
use hwall_core::{ReadingFreshness, RunningStatistics, Sensor, SensorStatus};

const EMPTY_VALUE: &str = "—";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SensorPresentation {
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
    pub dimmed: bool,
}

pub fn present_sensor(
    sensor: &Sensor,
    observed: Option<RunningStatistics>,
    rule: Option<&AlertRule>,
    state: AlertState,
) -> SensorPresentation {
    let rule = rule.filter(|rule| rule.is_configured() && alert_supported_sensor(sensor));
    let format_observed = |value: Option<f64>| {
        value
            .map(|value| format_value(value, &sensor.unit))
            .unwrap_or_else(|| EMPTY_VALUE.to_owned())
    };
    let color_for = |value: Option<f64>| {
        sensor.is_current().then_some(())?;
        rule.and_then(|rule| {
            rule.color_for(rule.severity_for_value(value?))
                .map(str::to_owned)
        })
    };

    SensorPresentation {
        current: sensor
            .value
            .map(|value| format_value(value, &sensor.unit))
            .or_else(|| sensor.raw_value.clone())
            .unwrap_or_else(|| EMPTY_VALUE.to_owned()),
        minimum: format_observed(observed.map(|value| value.minimum)),
        maximum: format_observed(observed.map(|value| value.maximum)),
        average: format_observed(observed.map(|value| value.average)),
        status: sensor_status(sensor, rule, state),
        current_color: color_for(sensor.value),
        minimum_color: color_for(observed.map(|value| value.minimum)),
        maximum_color: color_for(observed.map(|value| value.maximum)),
        average_color: color_for(observed.map(|value| value.average)),
        status_color: if !sensor.is_current()
            || matches!(
                sensor.status,
                SensorStatus::Fault | SensorStatus::Unavailable
            ) {
            None
        } else {
            rule.and_then(|rule| rule.color_for(state.severity()).map(str::to_owned))
        },
        dimmed: !sensor.is_current(),
    }
}

fn sensor_status(sensor: &Sensor, rule: Option<&AlertRule>, state: AlertState) -> String {
    match sensor.freshness {
        ReadingFreshness::Stale => {
            return sensor.last_updated_unix_ms.map_or_else(
                || "Stale".to_owned(),
                |timestamp| {
                    format!(
                        "Stale — last updated {}",
                        format_reading_age_compact(timestamp)
                    )
                },
            );
        }
        ReadingFreshness::Unavailable => return "Unavailable".to_owned(),
        ReadingFreshness::Offline => return "Offline".to_owned(),
        ReadingFreshness::Current => {}
    }
    match sensor.status {
        SensorStatus::Fault => return "Fault".to_owned(),
        SensorStatus::Unavailable => return "Unavailable".to_owned(),
        SensorStatus::Ok | SensorStatus::Alarm => {}
    }
    if let Some(sampled_at) = sensor.sampled_at_unix_ms() {
        return format!("Sampled {}", format_sample_age_compact(sampled_at));
    }
    if rule.is_some() && state != AlertState::Normal {
        return state.label().to_owned();
    }
    if sensor.has_unconfigured_hardware_alarm() {
        return "Limits not configured".to_owned();
    }
    match sensor.status {
        SensorStatus::Ok => "Normal".to_owned(),
        SensorStatus::Alarm => "Hardware alarm".to_owned(),
        SensorStatus::Fault => "Fault".to_owned(),
        SensorStatus::Unavailable => "Unavailable".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwall_core::{Identification, SensorKind, Unit};

    fn sensor(value: f64) -> Sensor {
        Sensor::new(
            "temp:0",
            "CPU temperature",
            SensorKind::Temperature,
            Unit::Celsius,
            Some(value),
            "/test/cpu",
            Identification::KernelLabel,
        )
    }

    #[test]
    fn normal_status_is_explicit() {
        assert_eq!(
            present_sensor(&sensor(42.0), None, None, AlertState::Normal).status,
            "Normal"
        );
    }

    #[test]
    fn stale_reading_is_dimmed_and_reports_last_update() {
        let mut stale = sensor(42.0);
        stale.freshness = ReadingFreshness::Stale;
        stale.last_updated_unix_ms = Some(0);
        let presentation = present_sensor(&stale, None, None, AlertState::Suspended);
        assert!(presentation.dimmed);
        assert!(presentation.status.starts_with("Stale — last updated "));
        assert_eq!(presentation.current, "42.0 °C");
    }

    #[test]
    fn unavailable_reading_is_dimmed() {
        let mut unavailable = sensor(42.0);
        unavailable.freshness = ReadingFreshness::Unavailable;
        let presentation = present_sensor(&unavailable, None, None, AlertState::Suspended);
        assert!(presentation.dimmed);
        assert_eq!(presentation.status, "Unavailable");
    }

    #[test]
    fn offline_reading_is_dimmed() {
        let mut offline = sensor(42.0);
        offline.freshness = ReadingFreshness::Offline;
        let presentation = present_sensor(&offline, None, None, AlertState::Suspended);
        assert!(presentation.dimmed);
        assert_eq!(presentation.status, "Offline");
    }

    #[test]
    fn unconfigured_hardware_alarm_is_informational() {
        let mut sensor = sensor(0.992);
        sensor
            .metadata
            .insert("alarm_note".to_owned(), "limits unavailable".into());

        assert_eq!(
            present_sensor(&sensor, None, None, AlertState::Normal).status,
            "Limits not configured"
        );
    }

    #[test]
    fn user_alert_takes_precedence_over_hardware_alarm_note() {
        let mut sensor = sensor(95.0);
        sensor
            .metadata
            .insert("alarm_note".to_owned(), "limits unavailable".into());
        let rule = AlertRule {
            critical_above: Some(90.0),
            ..AlertRule::default()
        };

        assert_eq!(
            present_sensor(&sensor, None, Some(&rule), AlertState::Critical).status,
            "Critical"
        );
    }
}
