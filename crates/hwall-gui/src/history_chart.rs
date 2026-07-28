use crate::history::{SensorKey, SharedHistory, MAX_HISTORY_RETENTION, MIN_HISTORY_RETENTION};
use gtk::cairo;
use gtk::prelude::*;
use gtk::{
    glib, Align, Button, CheckButton, ComboBoxText, DrawingArea, Label, Orientation, SpinButton,
};
use hwall_app::{default_log_directory, timestamped_log_path, LogFileWriter, LogFormat, SensorRow};
use hwall_core::render::format_value;
use hwall_core::Unit;
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

pub(super) fn panel(
    history: SharedHistory,
    key: SensorKey,
    unit: Unit,
    row_template: SensorRow,
    default_format: LogFormat,
    on_recording_changed: impl Fn() + 'static,
    on_exported: impl Fn(Result<PathBuf, String>) + 'static,
) -> gtk::Box {
    let global_retention = history.borrow().global_retention();
    let existing_recording = history.borrow().recording(&key);
    let recording_state = existing_recording.unwrap_or(crate::history::ExtendedRecording {
        retention: global_retention,
        persistent: false,
    });
    if existing_recording.is_none() {
        history.borrow_mut().start_extended(
            &key,
            recording_state.retention,
            recording_state.persistent,
        );
    }

    let view_window = Rc::new(Cell::new(global_retention));
    let root = gtk::Box::new(Orientation::Vertical, 8);
    root.set_margin_start(12);
    root.set_margin_end(12);
    root.set_margin_bottom(10);

    let chart = DrawingArea::builder()
        .content_width(600)
        .content_height(260)
        .hexpand(true)
        .vexpand(false)
        .build();
    chart.add_css_class("history-chart");
    install_draw_func(
        &chart,
        Rc::clone(&history),
        key.clone(),
        Rc::clone(&view_window),
        unit,
    );
    root.append(&chart);

    let view_row = gtk::Box::new(Orientation::Horizontal, 10);
    view_row.set_halign(Align::Fill);
    let chart_for_view = chart.clone();
    let view_window_for_change = Rc::clone(&view_window);
    let view = DurationSelector::new(
        "View",
        global_retention,
        MAX_HISTORY_RETENTION,
        move |duration| {
            view_window_for_change.set(duration);
            chart_for_view.queue_draw();
        },
    );
    view_row.append(&view.widget);

    let all = Button::with_label("All available");
    let history_for_all = Rc::clone(&history);
    let key_for_all = key.clone();
    let view_for_all = view.clone();
    all.connect_clicked(move |_| {
        let history = history_for_all.borrow();
        let available = history.available_duration(&key_for_all);
        view_for_all.set_duration(available.max(history.global_retention()));
    });
    view_row.append(&all);
    root.append(&view_row);

    let recording_row = gtk::Box::new(Orientation::Horizontal, 10);
    let recording = CheckButton::with_label("Extended recording");
    recording.set_active(true);
    recording_row.append(&recording);

    let history_for_retention = Rc::clone(&history);
    let key_for_retention = key.clone();
    let recording_for_retention = recording.clone();
    let retention = DurationSelector::new(
        "Keep",
        recording_state.retention,
        MAX_HISTORY_RETENTION,
        move |duration| {
            if recording_for_retention.is_active() {
                let persistent = history_for_retention
                    .borrow()
                    .recording(&key_for_retention)
                    .is_some_and(|recording| recording.persistent);
                history_for_retention.borrow_mut().start_extended(
                    &key_for_retention,
                    duration,
                    persistent,
                );
            }
        },
    );
    recording_row.append(&retention.widget);
    root.append(&recording_row);

    let persistent = CheckButton::with_label("Continue recording after closing");
    persistent.set_active(recording_state.persistent);
    persistent.set_margin_start(24);
    root.append(&persistent);

    let available = Label::new(None);
    available.set_xalign(0.0);
    available.add_css_class("dim-label");
    update_available_label(&available, &history, &key);
    root.append(&available);

    let export_row = gtk::Box::new(Orientation::Horizontal, 10);
    let export_format = ComboBoxText::new();
    for format in LogFormat::ALL {
        export_format.append(Some(format.id()), format.display_name());
    }
    export_format.set_active_id(Some(default_format.id()));
    export_row.append(&Label::new(Some("Export")));
    export_row.append(&export_format);

    let export_range = ComboBoxText::new();
    export_range.append(Some("all"), "All available");
    export_range.append(Some("view"), "Current view");
    export_range.set_active_id(Some("all"));
    export_row.append(&export_range);

    let export = Button::with_label("Export");
    export_row.append(&export);
    root.append(&export_row);

    let changed: Rc<dyn Fn()> = Rc::new(on_recording_changed);
    let history_for_toggle = Rc::clone(&history);
    let key_for_toggle = key.clone();
    let retention_for_toggle = retention.clone();
    let persistent_for_toggle = persistent.clone();
    let changed_for_toggle = Rc::clone(&changed);
    recording.connect_toggled(move |button| {
        let active = button.is_active();
        retention_for_toggle.set_sensitive(active);
        persistent_for_toggle.set_sensitive(active);
        if active {
            history_for_toggle.borrow_mut().start_extended(
                &key_for_toggle,
                retention_for_toggle.duration(),
                persistent_for_toggle.is_active(),
            );
        } else {
            history_for_toggle
                .borrow_mut()
                .stop_extended(&key_for_toggle);
        }
        changed_for_toggle();
    });

    let history_for_persistent = Rc::clone(&history);
    let key_for_persistent = key.clone();
    let recording_for_persistent = recording.clone();
    let changed_for_persistent = Rc::clone(&changed);
    persistent.connect_toggled(move |button| {
        if recording_for_persistent.is_active() {
            history_for_persistent
                .borrow_mut()
                .set_persistent(&key_for_persistent, button.is_active());
            changed_for_persistent();
        }
    });

    let history_for_export = Rc::clone(&history);
    let key_for_export = key.clone();
    let view_window_for_export = Rc::clone(&view_window);
    let exported: Rc<dyn Fn(Result<PathBuf, String>)> = Rc::new(on_exported);
    export.connect_clicked(move |_| {
        let format = export_format
            .active_id()
            .as_deref()
            .and_then(LogFormat::from_id)
            .unwrap_or_default();
        let window = match export_range.active_id().as_deref() {
            Some("view") => Some(view_window_for_export.get()),
            _ => None,
        };
        let samples = history_for_export
            .borrow()
            .samples(&key_for_export, window, Instant::now());
        let path = timestamped_log_path(default_log_directory(), format);
        let result = export_samples(&path, format, &row_template, unit, &samples)
            .map(|()| path)
            .map_err(|error| error.to_string());
        exported(result);
    });

    changed();
    let weak_chart = chart.downgrade();
    let history_for_tick = Rc::clone(&history);
    let key_for_tick = key;
    glib::timeout_add_local(Duration::from_millis(500), move || {
        let Some(chart) = weak_chart.upgrade() else {
            return glib::ControlFlow::Break;
        };
        update_available_label(&available, &history_for_tick, &key_for_tick);
        chart.queue_draw();
        glib::ControlFlow::Continue
    });

    root
}

fn export_samples(
    path: &std::path::Path,
    format: LogFormat,
    template: &SensorRow,
    unit: Unit,
    samples: &[crate::history::HistorySample],
) -> std::io::Result<()> {
    let mut writer = LogFileWriter::create(path, format)?;
    for sample in samples {
        let mut row = template.clone();
        row.current = sample
            .value
            .map(|value| format_value(value, &unit))
            .unwrap_or_else(|| "—".to_owned());
        row.minimum.clear();
        row.maximum.clear();
        row.average.clear();
        row.status = sample.status.to_owned();
        writer.write_sample(sample.timestamp_ms, std::slice::from_ref(&row))?;
    }
    writer.flush()
}

fn update_available_label(label: &Label, history: &SharedHistory, key: &SensorKey) {
    let duration = history.borrow().available_duration(key);
    label.set_text(&format!("Available: {}", duration_label(duration)));
}

#[derive(Clone)]
pub(crate) struct DurationSelector {
    pub(crate) widget: gtk::Box,
    spin: SpinButton,
    unit: ComboBoxText,
    seconds: Rc<Cell<u64>>,
    updating: Rc<Cell<bool>>,
    max_seconds: u64,
    callback: Rc<dyn Fn(Duration)>,
}

impl DurationSelector {
    pub(crate) fn new(
        label: &str,
        initial: Duration,
        maximum: Duration,
        on_changed: impl Fn(Duration) + 'static,
    ) -> Self {
        let widget = gtk::Box::new(Orientation::Horizontal, 6);
        if !label.is_empty() {
            widget.append(&Label::new(Some(label)));
        }

        let spin = SpinButton::with_range(1.0, 1_440.0, 1.0);
        spin.set_numeric(true);
        spin.set_width_chars(5);
        widget.append(&spin);

        let unit = ComboBoxText::new();
        unit.append(Some("minutes"), "minutes");
        unit.append(Some("hours"), "hours");
        widget.append(&unit);

        let seconds = Rc::new(Cell::new(
            initial
                .as_secs()
                .clamp(MIN_HISTORY_RETENTION.as_secs(), maximum.as_secs()),
        ));
        let updating = Rc::new(Cell::new(false));
        let callback: Rc<dyn Fn(Duration)> = Rc::new(on_changed);
        let selector = Self {
            widget,
            spin,
            unit,
            seconds: Rc::clone(&seconds),
            updating: Rc::clone(&updating),
            max_seconds: maximum.as_secs(),
            callback: Rc::clone(&callback),
        };
        selector.write_controls(
            initial
                .as_secs()
                .clamp(MIN_HISTORY_RETENTION.as_secs(), maximum.as_secs()),
        );

        let unit_for_spin = selector.unit.downgrade();
        let seconds_for_spin = Rc::clone(&seconds);
        let updating_for_spin = Rc::clone(&updating);
        let callback_for_spin = Rc::clone(&callback);
        let max_seconds = selector.max_seconds;
        selector.spin.connect_value_changed(move |spin| {
            if updating_for_spin.get() {
                return;
            }
            let Some(unit) = unit_for_spin.upgrade() else {
                return;
            };
            let seconds = read_controls(spin, &unit, max_seconds);
            seconds_for_spin.set(seconds);
            callback_for_spin(Duration::from_secs(seconds));
        });

        let spin_for_unit = selector.spin.downgrade();
        let seconds_for_unit = Rc::clone(&seconds);
        let updating_for_unit = Rc::clone(&updating);
        let callback_for_unit = Rc::clone(&callback);
        selector.unit.connect_changed(move |unit| {
            if updating_for_unit.get() {
                return;
            }
            let Some(spin) = spin_for_unit.upgrade() else {
                return;
            };
            let current = seconds_for_unit.get();
            write_active_unit(&spin, unit, &updating_for_unit, max_seconds, current);
            let normalized = read_controls(&spin, unit, max_seconds);
            seconds_for_unit.set(normalized);
            callback_for_unit(Duration::from_secs(normalized));
        });

        selector
    }

    pub(crate) fn duration(&self) -> Duration {
        Duration::from_secs(self.seconds.get())
    }

    fn set_duration(&self, duration: Duration) {
        let seconds = duration
            .as_secs()
            .clamp(MIN_HISTORY_RETENTION.as_secs(), self.max_seconds);
        self.write_controls(seconds);
        let normalized = self.read_controls();
        self.seconds.set(normalized);
        (self.callback)(Duration::from_secs(normalized));
    }

    fn set_sensitive(&self, sensitive: bool) {
        self.widget.set_sensitive(sensitive);
    }

    fn read_controls(&self) -> u64 {
        read_controls(&self.spin, &self.unit, self.max_seconds)
    }

    fn write_controls(&self, seconds: u64) {
        write_controls(
            &self.spin,
            &self.unit,
            &self.updating,
            self.max_seconds,
            seconds,
        );
    }
}

fn read_controls(spin: &SpinButton, unit: &ComboBoxText, max_seconds: u64) -> u64 {
    let multiplier = if unit.active_id().as_deref() == Some("hours") {
        3_600
    } else {
        60
    };
    (spin.value_as_int().max(1) as u64 * multiplier).min(max_seconds)
}

fn write_active_unit(
    spin: &SpinButton,
    unit: &ComboBoxText,
    updating: &Cell<bool>,
    max_seconds: u64,
    seconds: u64,
) {
    updating.set(true);
    if unit.active_id().as_deref() == Some("hours") {
        spin.set_range(1.0, (max_seconds / 3_600).max(1) as f64);
        spin.set_value(seconds.div_ceil(3_600) as f64);
    } else {
        spin.set_range(1.0, (max_seconds / 60).max(1) as f64);
        spin.set_value(seconds.div_ceil(60) as f64);
    }
    updating.set(false);
}

fn write_controls(
    spin: &SpinButton,
    unit: &ComboBoxText,
    updating: &Cell<bool>,
    max_seconds: u64,
    seconds: u64,
) {
    updating.set(true);
    if seconds >= 3_600 && seconds.is_multiple_of(3_600) {
        unit.set_active_id(Some("hours"));
        spin.set_range(1.0, (max_seconds / 3_600).max(1) as f64);
        spin.set_value((seconds / 3_600) as f64);
    } else {
        unit.set_active_id(Some("minutes"));
        spin.set_range(1.0, (max_seconds / 60).max(1) as f64);
        spin.set_value(seconds.div_ceil(60) as f64);
    }
    updating.set(false);
}

fn install_draw_func(
    chart: &DrawingArea,
    history: SharedHistory,
    key: SensorKey,
    view_window: Rc<Cell<Duration>>,
    unit: Unit,
) {
    chart.set_draw_func(move |_area, context, width, height| {
        let width = f64::from(width);
        let height = f64::from(height);
        if width < 120.0 || height < 100.0 {
            return;
        }

        let left = 68.0;
        let right = width - 12.0;
        let top = 12.0;
        let bottom = height - 30.0;
        let plot_width = (right - left).max(1.0);
        let plot_height = (bottom - top).max(1.0);

        context.set_source_rgba(0.5, 0.5, 0.5, 0.20);
        context.set_line_width(1.0);
        for index in 0..=4 {
            let y = top + plot_height * f64::from(index) / 4.0;
            context.move_to(left, y);
            context.line_to(right, y);
        }
        let _ = context.stroke();

        let max_points = plot_width.max(64.0) as usize;
        let points =
            history
                .borrow()
                .chart_points(&key, view_window.get(), Instant::now(), max_points);
        let mut values = points.iter().filter_map(|point| point.value);
        let Some(first) = values.next() else {
            draw_empty(context, left, top + plot_height / 2.0);
            draw_time_labels(context, left, right, bottom, view_window.get());
            return;
        };
        let (mut minimum, mut maximum) = values.fold((first, first), |bounds, value| {
            (bounds.0.min(value), bounds.1.max(value))
        });
        if (maximum - minimum).abs() < f64::EPSILON {
            let padding = (minimum.abs() * 0.05).max(1.0);
            minimum -= padding;
            maximum += padding;
        } else {
            let padding = (maximum - minimum) * 0.05;
            minimum -= padding;
            maximum += padding;
        }

        context.set_source_rgba(0.20, 0.52, 0.88, 0.95);
        context.set_line_width(1.5);
        let mut active_path = false;
        for point in points {
            let Some(value) = point.value else {
                if active_path {
                    let _ = context.stroke();
                    active_path = false;
                }
                continue;
            };
            let x = left + point.x * plot_width;
            let y = bottom - (value - minimum) / (maximum - minimum) * plot_height;
            if active_path {
                context.line_to(x, y);
            } else {
                context.move_to(x, y);
                active_path = true;
            }
        }
        if active_path {
            let _ = context.stroke();
        }

        draw_value_labels(context, top, bottom, minimum, maximum, unit);
        draw_time_labels(context, left, right, bottom, view_window.get());
    });
}

fn draw_empty(context: &cairo::Context, x: f64, y: f64) {
    context.set_source_rgba(0.5, 0.5, 0.5, 0.85);
    context.set_font_size(13.0);
    context.move_to(x, y);
    let _ = context.show_text("No numeric history yet");
}

fn draw_value_labels(
    context: &cairo::Context,
    top: f64,
    bottom: f64,
    minimum: f64,
    maximum: f64,
    unit: Unit,
) {
    context.set_source_rgba(0.5, 0.5, 0.5, 0.90);
    context.set_font_size(11.0);
    context.move_to(4.0, top + 10.0);
    let _ = context.show_text(&format_value(maximum, &unit));
    context.move_to(4.0, bottom);
    let _ = context.show_text(&format_value(minimum, &unit));
}

fn draw_time_labels(
    context: &cairo::Context,
    left: f64,
    right: f64,
    bottom: f64,
    window: Duration,
) {
    context.set_source_rgba(0.5, 0.5, 0.5, 0.90);
    context.set_font_size(11.0);
    context.move_to(left, bottom + 18.0);
    let _ = context.show_text(&format!("−{}", duration_label(window)));
    context.move_to(right - 24.0, bottom + 18.0);
    let _ = context.show_text("now");
}

fn duration_label(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds >= 3_600 && seconds.is_multiple_of(3_600) {
        let hours = seconds / 3_600;
        format!("{hours} h")
    } else {
        let minutes = seconds.div_ceil(60);
        format!("{minutes} min")
    }
}
