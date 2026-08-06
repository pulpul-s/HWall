use crate::ui::{attach_labeled, set_label_text};
use gtk::prelude::*;
use gtk::{CheckButton, Dialog, Grid, Label, ResponseType, Window};
use hwall_app::{
    AlertDirection, AlertRule, DEFAULT_CRITICAL_COLOR, DEFAULT_WARNING_COLOR, alert_direction,
    unit_suffix, valid_alert_color,
};
use hwall_core::{SensorKind, Unit};

pub(super) fn show(
    parent: &Window,
    sensor_name: &str,
    kind: SensorKind,
    unit: Unit,
    current: Option<AlertRule>,
    on_apply: impl Fn(Option<AlertRule>) + 'static,
) {
    let dialog = Dialog::builder()
        .title(format!("Alert for {sensor_name}"))
        .transient_for(parent)
        .modal(true)
        .use_header_bar(1)
        .default_width(520)
        .build();
    dialog.add_button("Cancel", ResponseType::Cancel);
    if current.is_some() {
        dialog.add_button("Disable alert", ResponseType::Other(1));
    }
    dialog.add_button("Apply", ResponseType::Apply);
    dialog.set_default_response(ResponseType::Apply);

    let rule = current.unwrap_or_default();
    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.set_margin_top(14);
    body.set_margin_bottom(14);
    body.set_margin_start(14);
    body.set_margin_end(14);

    let explanation = Label::new(Some(
        "Current readings drive alert state and notifications. Session statistics are colored for reference only.",
    ));
    explanation.set_xalign(0.0);
    explanation.set_wrap(true);
    explanation.add_css_class("dim-label");
    body.append(&explanation);

    let grid = Grid::builder().column_spacing(16).row_spacing(8).build();
    let direction = alert_direction(kind);
    let suffix = unit_suffix(unit);
    let mut row = 0;

    let warning_below = threshold_entry(rule.warning_below);
    let critical_below = threshold_entry(rule.critical_below);
    let warning_above = threshold_entry(rule.warning_above);
    let critical_above = threshold_entry(rule.critical_above);

    if matches!(direction, AlertDirection::Below | AlertDirection::Both) {
        attach_labeled(
            &grid,
            row,
            &threshold_label("Warning below", suffix),
            &warning_below,
        );
        row += 1;
        attach_labeled(
            &grid,
            row,
            &threshold_label("Critical below", suffix),
            &critical_below,
        );
        row += 1;
    }
    if matches!(direction, AlertDirection::Above | AlertDirection::Both) {
        attach_labeled(
            &grid,
            row,
            &threshold_label("Warning above", suffix),
            &warning_above,
        );
        row += 1;
        attach_labeled(
            &grid,
            row,
            &threshold_label("Critical above", suffix),
            &critical_above,
        );
        row += 1;
    }

    let duration = gtk::SpinButton::with_range(0.0, 3_600.0, 1.0);
    duration.set_value(rule.duration_seconds as f64);
    duration.set_tooltip_text(Some(
        "The current reading must remain beyond a limit for this long before the alert activates",
    ));
    attach_labeled(&grid, row, "Duration (seconds)", &duration);
    row += 1;

    let hysteresis = gtk::SpinButton::with_range(0.0, 1_000_000_000_000_000.0, 0.1);
    hysteresis.set_digits(2);
    hysteresis.set_value(rule.hysteresis.max(0.0));
    hysteresis.set_tooltip_text(Some(
        "How far the current reading must move back past a threshold before the alert clears",
    ));
    attach_labeled(
        &grid,
        row,
        &threshold_label("Hysteresis", suffix),
        &hysteresis,
    );
    row += 1;

    let cooldown = gtk::SpinButton::with_range(0.0, 86_400.0, 10.0);
    cooldown.set_value(rule.cooldown_seconds as f64);
    cooldown.set_tooltip_text(Some(
        "Suppresses repeated activation notifications after an alert has already been reported",
    ));
    attach_labeled(&grid, row, "Notification cooldown (seconds)", &cooldown);
    body.append(&grid);

    let notifications = CheckButton::with_label("Show desktop notifications");
    notifications.set_active(rule.desktop_notifications);
    let recovery = CheckButton::with_label("Notify when the sensor returns to normal");
    recovery.set_active(rule.recovery_notifications);
    recovery.set_sensitive(rule.desktop_notifications);
    let recovery_for_toggle = recovery.clone();
    notifications.connect_toggled(move |button| {
        recovery_for_toggle.set_sensitive(button.is_active());
    });
    body.append(&notifications);
    body.append(&recovery);

    let colors = gtk::Expander::new(Some("Custom colors"));
    colors.set_expanded(rule.warning_color.is_some() || rule.critical_color.is_some());
    let color_grid = Grid::builder().column_spacing(10).row_spacing(8).build();

    let warning_default = CheckButton::with_label("Use default");
    warning_default.set_active(rule.warning_color.is_none());
    let warning_color = color_entry(rule.warning_color());
    warning_color.set_sensitive(!warning_default.is_active());
    let warning_swatch = color_swatch(rule.warning_color());
    connect_color_swatch(&warning_color, &warning_swatch);
    let warning_color_for_toggle = warning_color.clone();
    let warning_swatch_for_toggle = warning_swatch.clone();
    warning_default.connect_toggled(move |button| {
        let use_default = button.is_active();
        warning_color_for_toggle.set_sensitive(!use_default);
        if use_default {
            set_color_swatch(&warning_swatch_for_toggle, DEFAULT_WARNING_COLOR);
        } else {
            let color = warning_color_for_toggle.text();
            set_color_swatch(&warning_swatch_for_toggle, color.as_str());
        }
    });
    let warning_label = Label::new(Some("Warning"));
    warning_label.set_xalign(0.0);
    color_grid.attach(&warning_label, 0, 0, 1, 1);
    color_grid.attach(&warning_swatch, 1, 0, 1, 1);
    color_grid.attach(&warning_color, 2, 0, 1, 1);
    color_grid.attach(&warning_default, 3, 0, 1, 1);

    let critical_default = CheckButton::with_label("Use default");
    critical_default.set_active(rule.critical_color.is_none());
    let critical_color = color_entry(rule.critical_color());
    critical_color.set_sensitive(!critical_default.is_active());
    let critical_swatch = color_swatch(rule.critical_color());
    connect_color_swatch(&critical_color, &critical_swatch);
    let critical_color_for_toggle = critical_color.clone();
    let critical_swatch_for_toggle = critical_swatch.clone();
    critical_default.connect_toggled(move |button| {
        let use_default = button.is_active();
        critical_color_for_toggle.set_sensitive(!use_default);
        if use_default {
            set_color_swatch(&critical_swatch_for_toggle, DEFAULT_CRITICAL_COLOR);
        } else {
            let color = critical_color_for_toggle.text();
            set_color_swatch(&critical_swatch_for_toggle, color.as_str());
        }
    });
    let critical_label = Label::new(Some("Critical"));
    critical_label.set_xalign(0.0);
    color_grid.attach(&critical_label, 0, 1, 1, 1);
    color_grid.attach(&critical_swatch, 1, 1, 1, 1);
    color_grid.attach(&critical_color, 2, 1, 1, 1);
    color_grid.attach(&critical_default, 3, 1, 1, 1);
    colors.set_child(Some(&color_grid));
    body.append(&colors);

    let error = Label::new(None);
    error.set_xalign(0.0);
    error.set_wrap(true);
    error.add_css_class("error");
    body.append(&error);

    dialog.content_area().append(&body);
    dialog.connect_response(move |dialog, response| match response {
        ResponseType::Apply => {
            let parsed = (|| {
                let warning_below = parse_threshold(&warning_below, "Warning below")?;
                let critical_below = parse_threshold(&critical_below, "Critical below")?;
                let warning_above = parse_threshold(&warning_above, "Warning above")?;
                let critical_above = parse_threshold(&critical_above, "Critical above")?;
                validate_threshold_order(
                    warning_below,
                    critical_below,
                    warning_above,
                    critical_above,
                )?;
                let updated = AlertRule {
                    warning_below,
                    critical_below,
                    warning_above,
                    critical_above,
                    duration_seconds: duration.value().round().max(0.0) as u64,
                    hysteresis: hysteresis.value().max(0.0),
                    cooldown_seconds: cooldown.value().round().max(0.0) as u64,
                    desktop_notifications: notifications.is_active(),
                    recovery_notifications: recovery.is_active(),
                    warning_color: parse_color(
                        &warning_color,
                        warning_default.is_active(),
                        "Warning color",
                    )?,
                    critical_color: parse_color(
                        &critical_color,
                        critical_default.is_active(),
                        "Critical color",
                    )?,
                };
                if !updated.is_configured() {
                    return Err("Enter at least one threshold or use Disable alert.".to_owned());
                }
                Ok(updated)
            })();
            match parsed {
                Ok(updated) => {
                    on_apply(Some(updated));
                    dialog.close();
                }
                Err(message) => error.set_text(&message),
            }
        }
        ResponseType::Other(1) => {
            on_apply(None);
            dialog.close();
        }
        _ => dialog.close(),
    });
    dialog.present();
}

fn threshold_entry(value: Option<f64>) -> gtk::Entry {
    let entry = gtk::Entry::new();
    entry.set_placeholder_text(Some("Not set"));
    entry.set_input_purpose(gtk::InputPurpose::Number);
    entry.set_activates_default(true);
    if let Some(value) = value {
        entry.set_text(&value.to_string());
    }
    entry
}

fn threshold_label(label: &str, suffix: &str) -> String {
    if suffix.is_empty() {
        label.to_owned()
    } else {
        format!("{label} ({suffix})")
    }
}

fn parse_threshold(entry: &gtk::Entry, label: &str) -> Result<Option<f64>, String> {
    let value = entry.text();
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    value
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .map(Some)
        .ok_or_else(|| format!("{label} must be a finite number."))
}

fn validate_threshold_order(
    warning_below: Option<f64>,
    critical_below: Option<f64>,
    warning_above: Option<f64>,
    critical_above: Option<f64>,
) -> Result<(), String> {
    if let (Some(warning), Some(critical)) = (warning_below, critical_below)
        && critical > warning
    {
        return Err("Critical below must not be higher than Warning below.".to_owned());
    }
    if let (Some(warning), Some(critical)) = (warning_above, critical_above)
        && critical < warning
    {
        return Err("Critical above must not be lower than Warning above.".to_owned());
    }
    if let (Some(below), Some(above)) = (warning_below, warning_above)
        && below >= above
    {
        return Err("Warning below must be lower than Warning above.".to_owned());
    }
    if let (Some(below), Some(above)) = (critical_below, critical_above)
        && below >= above
    {
        return Err("Critical below must be lower than Critical above.".to_owned());
    }
    Ok(())
}

fn color_entry(value: &str) -> gtk::Entry {
    let entry = gtk::Entry::new();
    entry.set_width_chars(9);
    entry.set_max_length(7);
    entry.set_text(value);
    entry.set_tooltip_text(Some("Hex color in #RRGGBB format"));
    entry
}

fn color_swatch(value: &str) -> Label {
    let label = Label::new(None);
    label.set_width_chars(2);
    set_color_swatch(&label, value);
    label
}

fn connect_color_swatch(entry: &gtk::Entry, swatch: &Label) {
    let swatch = swatch.clone();
    entry.connect_changed(move |entry| {
        set_color_swatch(&swatch, entry.text().as_str());
    });
}

fn set_color_swatch(label: &Label, color: &str) {
    if valid_alert_color(color) {
        set_label_text(label, "■", Some(color));
    } else {
        set_label_text(label, "□", None);
    }
}

fn parse_color(
    entry: &gtk::Entry,
    use_default: bool,
    label: &str,
) -> Result<Option<String>, String> {
    if use_default {
        return Ok(None);
    }
    let color = entry.text().trim().to_ascii_lowercase();
    if valid_alert_color(&color) {
        Ok(Some(color))
    } else {
        Err(format!("{label} must use #RRGGBB format."))
    }
}
