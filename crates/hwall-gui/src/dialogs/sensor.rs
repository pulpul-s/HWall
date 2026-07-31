use super::*;

struct SensorValueLabels {
    current: Label,
    status: Label,
    samples: Label,
    minimum: Label,
    maximum: Label,
    average: Label,
}

impl SensorValueLabels {
    fn update(
        &self,
        sensor: &Sensor,
        observed: Option<RunningStatistics>,
        rule: Option<&AlertRule>,
        state: AlertState,
    ) {
        let presentation = present_sensor(sensor, observed, rule, state);
        set_label_text(
            &self.current,
            &presentation.current,
            presentation.current_color.as_deref(),
        );
        set_label_text(
            &self.status,
            &presentation.status,
            presentation.status_color.as_deref(),
        );
        for label in [&self.current, &self.status] {
            if presentation.dimmed {
                label.add_css_class("stale-cell");
            } else {
                label.remove_css_class("stale-cell");
            }
        }
        self.samples
            .set_text(&observed.map_or(0, |value| value.samples).to_string());
        set_label_text(
            &self.minimum,
            &presentation.minimum,
            presentation.minimum_color.as_deref(),
        );
        set_label_text(
            &self.maximum,
            &presentation.maximum,
            presentation.maximum_color.as_deref(),
        );
        set_label_text(
            &self.average,
            &presentation.average,
            presentation.average_color.as_deref(),
        );
    }

    fn set_unavailable(&self) {
        self.current.remove_css_class("stale-cell");
        self.status.remove_css_class("stale-cell");
        set_label_text(&self.current, "Unavailable", None);
        set_label_text(&self.status, "Unavailable", None);
        self.samples.set_text("0");
        for label in [&self.minimum, &self.maximum, &self.average] {
            set_label_text(label, "—", None);
        }
    }
}

pub(crate) type LiveSensorDetails = (
    Device,
    Sensor,
    Option<RunningStatistics>,
    Option<AlertRule>,
    AlertState,
);

pub(crate) struct SensorDetailsRequest {
    pub device: Device,
    pub sensor: Sensor,
    pub observed: Option<RunningStatistics>,
    pub alert_rule: Option<AlertRule>,
    pub alert_state: AlertState,
    pub row: SensorRow,
    pub history: SharedHistory,
    pub history_key: SensorKey,
    pub default_log_format: LogFormat,
    pub export_directory: Rc<RefCell<PathBuf>>,
    pub read_live: Box<dyn Fn() -> Option<LiveSensorDetails>>,
    pub configure_alert: Box<dyn Fn(&Window)>,
    pub recording_changed: Rc<dyn Fn()>,
    pub exported: Box<dyn Fn(Result<PathBuf, String>)>,
}

pub(crate) fn show_sensor_details(
    parent: &ApplicationWindow,
    request: SensorDetailsRequest,
) -> Window {
    let SensorDetailsRequest {
        device,
        sensor,
        observed,
        alert_rule,
        alert_state,
        row: row_template,
        history,
        history_key,
        default_log_format,
        export_directory,
        read_live,
        configure_alert: on_configure_alert,
        recording_changed,
        exported,
    } = request;

    let window = details_window(parent, &row_template.label, 700, 720);
    let list = details_list();
    let alerts_supported = alert_supported_sensor(&sensor);
    let alert_rule = alert_rule.filter(|rule| alerts_supported && rule.is_configured());

    if crate::history::chartable_unit(sensor.unit) && !sensor.is_intermittent() {
        append_section(&list, "History");
        let changed_for_panel = Rc::clone(&recording_changed);
        list.append(&crate::history_chart::panel(
            Rc::clone(&history),
            history_key.clone(),
            sensor.unit,
            row_template,
            default_log_format,
            move || changed_for_panel(),
            crate::history_chart::ExportContext {
                parent: window.clone(),
                directory: export_directory,
                on_exported: exported,
            },
        ));
    }

    append_section(&list, "Current reading");
    let device_value = append_detail_value(&list, "Device", &device.name);
    append_detail(&list, "Sensor ID", &sensor.id);
    append_detail(&list, "Kind", sensor_kind_name(sensor.kind));
    let values = SensorValueLabels {
        current: append_detail_value(&list, "Current", "—"),
        status: append_detail_value(&list, "Status", "Normal"),
        samples: {
            append_section(&list, "Observed this session");
            append_detail_value(&list, "Samples", "0")
        },
        minimum: append_detail_value(&list, "Minimum", "—"),
        maximum: append_detail_value(&list, "Maximum", "—"),
        average: append_detail_value(&list, "Average", "—"),
    };
    values.update(&sensor, observed, alert_rule.as_ref(), alert_state);

    let alert_widgets = if alerts_supported {
        append_section(&list, "Alert");
        let alert_rule_value = append_detail_value(
            &list,
            "Rule",
            &alert_rule
                .as_ref()
                .map(|rule| rule_summary(rule, sensor.unit))
                .unwrap_or_else(|| "Not configured".to_owned()),
        );
        let alert_state_value = append_detail_value(&list, "Current state", alert_state.label());
        set_label_text(
            &alert_state_value,
            alert_state.label(),
            alert_rule
                .as_ref()
                .and_then(|rule| rule.color_for(alert_state.severity())),
        );
        let configure_alert_button = gtk::Button::with_label(if alert_rule.is_some() {
            "Edit alert…"
        } else {
            "Configure alert…"
        });
        configure_alert_button.set_halign(Align::Start);
        configure_alert_button.set_margin_top(4);
        configure_alert_button.set_margin_bottom(8);
        configure_alert_button.set_margin_start(12);
        configure_alert_button.set_focus_on_click(false);
        let weak_window_for_alert = window.downgrade();
        configure_alert_button.connect_clicked(move |_| {
            if let Some(window) = weak_window_for_alert.upgrade() {
                on_configure_alert(&window);
            }
        });
        list.append(&configure_alert_button);
        Some((alert_rule_value, alert_state_value, configure_alert_button))
    } else {
        None
    };

    if sensor.min.is_some() || sensor.max.is_some() || sensor.critical.is_some() {
        append_section(&list, "Hardware limits");
        if let Some(value) = sensor.min {
            append_detail(&list, "Minimum", &format_value(value, &sensor.unit));
        }
        if let Some(value) = sensor.max {
            append_detail(&list, "Maximum", &format_value(value, &sensor.unit));
        }
        if let Some(value) = sensor.critical {
            append_detail(&list, "Critical", &format_value(value, &sensor.unit));
        }
    }

    append_section(&list, "Identification");
    append_detail(
        &list,
        "Identification",
        identification_name(sensor.identification),
    );
    append_detail(&list, "Source", &sensor.source);

    if !sensor.metadata.is_empty() {
        append_section(&list, "Metadata");
        for (key, value) in &sensor.metadata {
            if let Some(rendered) = format_property_value(key, value) {
                append_detail(&list, &humanize_key(key), &rendered);
            }
        }
    }

    let history_for_close = Rc::clone(&history);
    let key_for_close = history_key.clone();
    let changed_for_close = Rc::clone(&recording_changed);
    window.connect_close_request(move |_| {
        history_for_close.borrow_mut().close_details(&key_for_close);
        changed_for_close();
        glib::Propagation::Proceed
    });

    let weak_window = window.downgrade();
    glib::timeout_add_local(Duration::from_millis(500), move || {
        if weak_window.upgrade().is_none() {
            return glib::ControlFlow::Break;
        }
        if let Some((device, sensor, observed, alert_rule, alert_state)) = read_live() {
            let alert_rule =
                alert_rule.filter(|rule| alert_supported_sensor(&sensor) && rule.is_configured());
            device_value.set_text(&device.name);
            values.update(&sensor, observed, alert_rule.as_ref(), alert_state);
            if let Some((alert_rule_value, alert_state_value, configure_alert)) = &alert_widgets {
                alert_rule_value.set_text(
                    &alert_rule
                        .as_ref()
                        .map(|rule| rule_summary(rule, sensor.unit))
                        .unwrap_or_else(|| "Not configured".to_owned()),
                );
                set_label_text(
                    alert_state_value,
                    alert_state.label(),
                    alert_rule
                        .as_ref()
                        .and_then(|rule| rule.color_for(alert_state.severity())),
                );
                configure_alert.set_label(if alert_rule.is_some() {
                    "Edit alert…"
                } else {
                    "Configure alert…"
                });
            }
        } else {
            values.set_unavailable();
        }
        glib::ControlFlow::Continue
    });

    finish_details_window(&window, &list);
    window
}
