use crate::history::{
    ChartPoint, HistoryPoint, HistorySample, HistoryStore, MAX_HISTORY_RETENTION,
    MIN_HISTORY_RETENTION, SensorKey, SharedHistory, nearest_chart_point,
};
use gtk::cairo;
use gtk::prelude::*;
use gtk::{
    Align, Button, CheckButton, ComboBoxText, DrawingArea, EventControllerMotion,
    EventControllerScroll, EventControllerScrollFlags, FileChooserAction, FileChooserNative,
    GestureDrag, Label, Orientation, ResponseType, SpinButton, Window, glib,
};
use hwall_app::{LogFileWriter, LogFormat, SensorRow, timestamped_log_path};
use hwall_core::Unit;
use hwall_core::render::format_value;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PLOT_LEFT: f64 = 68.0;
const PLOT_RIGHT_MARGIN: f64 = 12.0;
const PLOT_TOP: f64 = 12.0;
const PLOT_BOTTOM_MARGIN: f64 = 30.0;
const ZOOM_STEP: f64 = 1.25;
const CHART_REFRESH_PERIOD: Duration = Duration::from_millis(100);
const EXPORT_RESULT_POLL_PERIOD: Duration = Duration::from_millis(50);

pub(super) struct ExportContext {
    pub parent: Window,
    pub directory: Rc<RefCell<PathBuf>>,
    pub on_exported: Box<dyn Fn(Result<PathBuf, String>)>,
}

struct HistoryExport {
    path: PathBuf,
    format: LogFormat,
    template: SensorRow,
    unit: Unit,
    samples: Vec<HistorySample>,
}

#[derive(Clone, Copy)]
enum ViewRange {
    Fixed(Duration),
    AllAvailable,
    Manual(ChartWindow),
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ChartWindow {
    duration: Duration,
    end: Instant,
}

#[derive(Default)]
struct ChartCache {
    revision: u64,
    window: Option<ChartWindow>,
    max_points: usize,
    points: Vec<ChartPoint>,
}

#[derive(Clone, Copy)]
struct PlotArea {
    left: f64,
    right: f64,
    top: f64,
    bottom: f64,
}

impl PlotArea {
    fn from_size(width: f64, height: f64) -> Option<Self> {
        (width >= 120.0 && height >= 100.0).then_some(Self {
            left: PLOT_LEFT,
            right: width - PLOT_RIGHT_MARGIN,
            top: PLOT_TOP,
            bottom: height - PLOT_BOTTOM_MARGIN,
        })
    }

    fn width(self) -> f64 {
        (self.right - self.left).max(1.0)
    }

    fn height(self) -> f64 {
        (self.bottom - self.top).max(1.0)
    }

    fn ratio(self, x: f64, y: f64) -> Option<f64> {
        if !(self.left..=self.right).contains(&x) || !(self.top..=self.bottom).contains(&y) {
            return None;
        }
        Some(((x - self.left) / self.width()).clamp(0.0, 1.0))
    }
}

impl ChartWindow {
    fn start(self) -> Instant {
        self.end.checked_sub(self.duration).unwrap_or(self.end)
    }

    fn contains(self, point: Instant) -> bool {
        point >= self.start() && point <= self.end
    }

    fn clamped(self, available: Option<(Instant, Duration)>, now: Instant) -> Self {
        let duration = self
            .duration
            .clamp(MIN_HISTORY_RETENTION, MAX_HISTORY_RETENTION);
        let Some((latest, available_duration)) = available else {
            return Self { duration, end: now };
        };
        if duration >= available_duration {
            return Self {
                duration,
                end: latest,
            };
        }
        let oldest = latest.checked_sub(available_duration).unwrap_or(latest);
        let earliest_end = oldest.checked_add(duration).unwrap_or(latest);
        Self {
            duration,
            end: self.end.clamp(earliest_end, latest),
        }
    }
}

impl ViewRange {
    fn window(self, available: Option<(Instant, Duration)>, now: Instant) -> ChartWindow {
        match self {
            Self::Fixed(duration) => ChartWindow { duration, end: now },
            Self::AllAvailable => available.map_or(
                ChartWindow {
                    duration: MIN_HISTORY_RETENTION,
                    end: now,
                },
                |(end, duration)| ChartWindow {
                    duration: duration.max(MIN_HISTORY_RETENTION),
                    end,
                },
            ),
            Self::Manual(window) => window.clamped(available, now),
        }
    }

    fn end_label(self) -> &'static str {
        match self {
            Self::Fixed(_) => "now",
            Self::AllAvailable => "latest",
            Self::Manual(_) => "end",
        }
    }
}

fn zoomed_window(
    window: ChartWindow,
    available: Option<(Instant, Duration)>,
    anchor: f64,
    factor: f64,
    now: Instant,
) -> ChartWindow {
    let anchor = anchor.clamp(0.0, 1.0);
    let duration = scaled_duration(window.duration, factor);
    let anchor_at = instant_at_ratio(window, anchor);
    let tail = duration.mul_f64(1.0 - anchor);
    let end = anchor_at.checked_add(tail).unwrap_or(anchor_at);
    ChartWindow { duration, end }.clamped(available, now)
}

fn scaled_duration(duration: Duration, factor: f64) -> Duration {
    let minutes = duration.as_secs_f64() / 60.0;
    let scaled = minutes * factor;
    let rounded = if factor >= 1.0 {
        scaled.ceil()
    } else {
        scaled.floor()
    };
    let minimum = MIN_HISTORY_RETENTION.as_secs() / 60;
    let maximum = MAX_HISTORY_RETENTION.as_secs() / 60;
    Duration::from_secs((rounded as u64).clamp(minimum, maximum) * 60)
}

fn panned_window(
    window: ChartWindow,
    available: Option<(Instant, Duration)>,
    offset_fraction: f64,
    now: Instant,
) -> ChartWindow {
    let offset = window
        .duration
        .mul_f64(offset_fraction.abs().clamp(0.0, 10.0));
    let end = if offset_fraction >= 0.0 {
        window.end.checked_sub(offset).unwrap_or(window.start())
    } else {
        window.end.checked_add(offset).unwrap_or(window.end)
    };
    ChartWindow {
        duration: window.duration,
        end,
    }
    .clamped(available, now)
}

fn instant_at_ratio(window: ChartWindow, ratio: f64) -> Instant {
    window
        .start()
        .checked_add(window.duration.mul_f64(ratio.clamp(0.0, 1.0)))
        .unwrap_or(window.end)
}

pub(super) fn panel(
    history: SharedHistory,
    key: SensorKey,
    unit: Unit,
    row_template: SensorRow,
    default_format: LogFormat,
    on_recording_changed: impl Fn() + 'static,
    export_context: ExportContext,
) -> gtk::Box {
    let ExportContext {
        parent,
        directory: export_directory,
        on_exported,
    } = export_context;
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

    let view_range = Rc::new(Cell::new(ViewRange::Fixed(global_retention)));
    let hover_ratio = Rc::new(Cell::new(None));
    let hover_point = Rc::new(Cell::new(None));
    let root = gtk::Box::new(Orientation::Vertical, 8);
    root.set_margin_start(12);
    root.set_margin_end(12);
    root.set_margin_bottom(10);

    let chart_header = gtk::Box::new(Orientation::Horizontal, 12);
    let range_label = Label::new(None);
    range_label.set_hexpand(true);
    range_label.set_xalign(0.0);
    range_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    range_label.add_css_class("dim-label");
    chart_header.append(&range_label);

    let hover_label = Label::new(None);
    hover_label.set_xalign(1.0);
    hover_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    hover_label.add_css_class("dim-label");
    chart_header.append(&hover_label);
    root.append(&chart_header);

    let chart = DrawingArea::builder()
        .content_width(600)
        .content_height(260)
        .hexpand(true)
        .vexpand(false)
        .build();
    chart.add_css_class("history-chart");
    root.append(&chart);
    let chart_cache = Rc::new(RefCell::new(ChartCache::default()));

    let view_row = gtk::Box::new(Orientation::Horizontal, 10);
    view_row.set_halign(Align::Fill);
    let chart_for_view = chart.clone();
    let history_for_view = Rc::clone(&history);
    let key_for_view = key.clone();
    let view_range_for_change = Rc::clone(&view_range);
    let hover_ratio_for_view = Rc::clone(&hover_ratio);
    let hover_point_for_view = Rc::clone(&hover_point);
    let range_label_for_view = range_label.clone();
    let hover_label_for_view = hover_label.clone();
    let view = DurationSelector::new(
        "View",
        global_retention,
        MAX_HISTORY_RETENTION,
        move |duration| {
            let range = ViewRange::Fixed(duration);
            view_range_for_change.set(range);
            clear_hover(
                &hover_ratio_for_view,
                &hover_point_for_view,
                &hover_label_for_view,
            );
            update_range_label(
                &range_label_for_view,
                &history_for_view,
                &key_for_view,
                range,
            );
            chart_for_view.queue_draw();
        },
    );
    view_row.append(&view.widget);

    let all = Button::with_label("All available");
    let history_for_all = Rc::clone(&history);
    let key_for_all = key.clone();
    let view_for_all = view.clone();
    let view_range_for_all = Rc::clone(&view_range);
    let hover_ratio_for_all = Rc::clone(&hover_ratio);
    let hover_point_for_all = Rc::clone(&hover_point);
    let range_label_for_all = range_label.clone();
    let hover_label_for_all = hover_label.clone();
    let chart_for_all = chart.clone();
    all.connect_clicked(move |_| {
        let window = ViewRange::AllAvailable.window(
            history_for_all.borrow().available_range(&key_for_all),
            Instant::now(),
        );
        view_range_for_all.set(ViewRange::AllAvailable);
        view_for_all.set_display_duration(window.duration);
        clear_hover(
            &hover_ratio_for_all,
            &hover_point_for_all,
            &hover_label_for_all,
        );
        update_range_label(
            &range_label_for_all,
            &history_for_all,
            &key_for_all,
            ViewRange::AllAvailable,
        );
        chart_for_all.queue_draw();
    });
    view_row.append(&all);
    root.append(&view_row);

    install_draw_func(
        &chart,
        Rc::clone(&history),
        key.clone(),
        Rc::clone(&view_range),
        Rc::clone(&hover_ratio),
        Rc::clone(&chart_cache),
        unit,
    );
    let interaction = InteractionState {
        history: Rc::clone(&history),
        key: key.clone(),
        view_range: Rc::clone(&view_range),
        hover_ratio: Rc::clone(&hover_ratio),
        hover_point: Rc::clone(&hover_point),
        chart_cache: Rc::clone(&chart_cache),
        range_label: range_label.clone(),
        hover_label: hover_label.clone(),
        view: view.clone(),
        unit,
    };
    install_interactions(&chart, interaction.clone());
    update_range_label(&range_label, &history, &key, view_range.get());

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
    let view_range_for_export = Rc::clone(&view_range);
    let exported: Rc<dyn Fn(Result<PathBuf, String>)> = Rc::from(on_exported);
    let export_button = export.clone();
    let weak_parent = parent.downgrade();
    export.connect_clicked(move |_| {
        let Some(parent) = weak_parent.upgrade() else {
            return;
        };
        let format = export_format
            .active_id()
            .as_deref()
            .and_then(LogFormat::from_id)
            .unwrap_or_default();
        let export_current_view = export_range.active_id().as_deref() == Some("view");
        let directory = export_directory.borrow().clone();
        if let Err(error) = std::fs::create_dir_all(&directory) {
            exported(Err(format!(
                "could not prepare export directory {}: {error}",
                directory.display()
            )));
            return;
        }

        let chooser = FileChooserNative::new(
            Some("Save sensor history"),
            Some(&parent),
            FileChooserAction::Save,
            Some("_Save"),
            Some("_Cancel"),
        );
        chooser.set_modal(true);
        chooser.set_current_name(&default_export_filename(format));
        let folder = gtk::gio::File::for_path(&directory);
        let _ = chooser.set_current_folder(Some(&folder));
        export_button.set_sensitive(false);

        let history_for_response = Rc::clone(&history_for_export);
        let key_for_response = key_for_export.clone();
        let view_range_for_response = Rc::clone(&view_range_for_export);
        let export_directory_for_response = Rc::clone(&export_directory);
        let exported_for_response = Rc::clone(&exported);
        let export_for_response = export_button.clone();
        let template = row_template.clone();
        chooser.run_async(move |chooser, response| {
            let selected_path = chooser.file().and_then(|file| file.path());
            chooser.destroy();
            if response != ResponseType::Accept {
                export_for_response.set_sensitive(true);
                return;
            }
            let Some(path) = selected_path else {
                export_for_response.set_sensitive(true);
                exported_for_response(Err("no export destination was selected".to_owned()));
                return;
            };
            let path = ensure_export_extension(path, format);
            if let Some(parent) = path.parent() {
                *export_directory_for_response.borrow_mut() = parent.to_path_buf();
            }

            let samples = selected_export_samples(
                &history_for_response,
                &key_for_response,
                view_range_for_response.get(),
                export_current_view,
            );
            start_history_export(
                HistoryExport {
                    path,
                    format,
                    template,
                    unit,
                    samples,
                },
                export_for_response,
                exported_for_response,
            );
        });
    });

    changed();
    let weak_chart = chart.downgrade();
    let history_for_tick = Rc::clone(&history);
    let view_range_for_tick = Rc::clone(&view_range);
    let interaction_for_tick = interaction;
    let view_for_tick = view.clone();
    let key_for_tick = key;
    let mut last_revision = history_for_tick.borrow().revision();
    glib::timeout_add_local(CHART_REFRESH_PERIOD, move || {
        let Some(chart) = weak_chart.upgrade() else {
            return glib::ControlFlow::Break;
        };
        let revision = history_for_tick.borrow().revision();
        if revision == last_revision {
            return glib::ControlFlow::Continue;
        }
        last_revision = revision;

        if matches!(view_range_for_tick.get(), ViewRange::AllAvailable) {
            let window = ViewRange::AllAvailable.window(
                history_for_tick.borrow().available_range(&key_for_tick),
                Instant::now(),
            );
            view_for_tick.set_display_duration(window.duration);
        }
        refresh_range_label(&interaction_for_tick);
        refresh_hover_state(&interaction_for_tick, &chart);
        update_available_label(&available, &history_for_tick, &key_for_tick);
        chart.queue_draw();
        glib::ControlFlow::Continue
    });

    root
}

fn selected_export_samples(
    history: &SharedHistory,
    key: &SensorKey,
    view_range: ViewRange,
    current_view_only: bool,
) -> Vec<HistorySample> {
    let now = Instant::now();
    let history = history.borrow();
    let chart_window = view_range.window(history.available_range(key), now);
    let (window, end) = if current_view_only {
        (Some(chart_window.duration), chart_window.end)
    } else {
        (None, now)
    };
    history.samples(key, window, end)
}

fn start_history_export(
    export: HistoryExport,
    button: Button,
    on_exported: Rc<dyn Fn(Result<PathBuf, String>)>,
) {
    let (result_tx, result_rx) = mpsc::channel();
    let spawn = thread::Builder::new()
        .name("hwall-history-export".to_owned())
        .spawn(move || {
            let HistoryExport {
                path,
                format,
                template,
                unit,
                samples,
            } = export;
            let result = export_samples(&path, format, &template, unit, &samples)
                .map(|()| path)
                .map_err(|error| error.to_string());
            let _ = result_tx.send(result);
        });
    if let Err(error) = spawn {
        button.set_sensitive(true);
        on_exported(Err(format!("could not start export worker: {error}")));
        return;
    }

    glib::timeout_add_local(EXPORT_RESULT_POLL_PERIOD, move || {
        let result = match result_rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("history export worker disconnected".to_owned())
            }
        };
        button.set_sensitive(true);
        on_exported(result);
        glib::ControlFlow::Break
    });
}

fn default_export_filename(format: LogFormat) -> String {
    timestamped_log_path(Path::new(""), format)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("hwall-history")
        .to_owned()
}

fn ensure_export_extension(mut path: PathBuf, format: LogFormat) -> PathBuf {
    if path.extension().is_none() {
        path.set_extension(format.id());
    }
    path
}

fn export_samples(
    path: &Path,
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
struct InteractionState {
    history: SharedHistory,
    key: SensorKey,
    view_range: Rc<Cell<ViewRange>>,
    hover_ratio: Rc<Cell<Option<f64>>>,
    hover_point: Rc<Cell<Option<HistoryPoint>>>,
    chart_cache: Rc<RefCell<ChartCache>>,
    range_label: Label,
    hover_label: Label,
    view: DurationSelector,
    unit: Unit,
}

fn install_interactions(chart: &DrawingArea, state: InteractionState) {
    let motion = EventControllerMotion::new();
    let weak_chart = chart.downgrade();
    let state_for_enter = state.clone();
    motion.connect_enter(move |_, x, y| {
        let Some(chart) = weak_chart.upgrade() else {
            return;
        };
        let ratio = PlotArea::from_size(f64::from(chart.width()), f64::from(chart.height()))
            .and_then(|plot| plot.ratio(x, y));
        state_for_enter.hover_ratio.set(ratio);
        refresh_hover_state(&state_for_enter, &chart);
        chart.queue_draw();
    });

    let weak_chart = chart.downgrade();
    let state_for_motion = state.clone();
    motion.connect_motion(move |_, x, y| {
        let Some(chart) = weak_chart.upgrade() else {
            return;
        };
        let ratio = PlotArea::from_size(f64::from(chart.width()), f64::from(chart.height()))
            .and_then(|plot| plot.ratio(x, y));
        state_for_motion.hover_ratio.set(ratio);
        refresh_hover_state(&state_for_motion, &chart);
        chart.queue_draw();
    });

    let weak_chart = chart.downgrade();
    let state_for_leave = state.clone();
    motion.connect_leave(move |_| {
        clear_hover(
            &state_for_leave.hover_ratio,
            &state_for_leave.hover_point,
            &state_for_leave.hover_label,
        );
        if let Some(chart) = weak_chart.upgrade() {
            chart.queue_draw();
        }
    });
    chart.add_controller(motion);

    let scroll = EventControllerScroll::new(EventControllerScrollFlags::VERTICAL);
    let weak_chart = chart.downgrade();
    let state_for_scroll = state.clone();
    scroll.connect_scroll(move |_, _dx, dy| {
        if dy.abs() < f64::EPSILON {
            return glib::Propagation::Proceed;
        }
        let Some(chart) = weak_chart.upgrade() else {
            return glib::Propagation::Proceed;
        };
        let now = Instant::now();
        let available = state_for_scroll
            .history
            .borrow()
            .available_range(&state_for_scroll.key);
        let Some(anchor) = state_for_scroll.hover_ratio.get() else {
            return glib::Propagation::Proceed;
        };
        let current = state_for_scroll.view_range.get().window(available, now);
        let factor = if dy > 0.0 {
            ZOOM_STEP
        } else {
            ZOOM_STEP.recip()
        };
        let window = zoomed_window(current, available, anchor, factor, now);
        state_for_scroll.view_range.set(ViewRange::Manual(window));
        state_for_scroll.view.set_display_duration(window.duration);
        refresh_range_label(&state_for_scroll);
        refresh_hover_state(&state_for_scroll, &chart);
        chart.queue_draw();
        glib::Propagation::Stop
    });
    chart.add_controller(scroll);

    let drag = GestureDrag::new();
    drag.set_button(1);
    let drag_origin = Rc::new(Cell::new(None));
    let weak_chart = chart.downgrade();
    let state_for_begin = state.clone();
    let drag_origin_for_begin = Rc::clone(&drag_origin);
    drag.connect_drag_begin(move |_, x, y| {
        let Some(chart) = weak_chart.upgrade() else {
            return;
        };
        let plot = PlotArea::from_size(f64::from(chart.width()), f64::from(chart.height()));
        if plot.and_then(|plot| plot.ratio(x, y)).is_none() {
            drag_origin_for_begin.set(None);
            return;
        }
        let now = Instant::now();
        let available = state_for_begin
            .history
            .borrow()
            .available_range(&state_for_begin.key);
        drag_origin_for_begin.set(Some(
            state_for_begin.view_range.get().window(available, now),
        ));
        clear_hover(
            &state_for_begin.hover_ratio,
            &state_for_begin.hover_point,
            &state_for_begin.hover_label,
        );
        chart.queue_draw();
    });

    let weak_chart = chart.downgrade();
    let state_for_update = state.clone();
    let drag_origin_for_update = Rc::clone(&drag_origin);
    drag.connect_drag_update(move |_, offset_x, _offset_y| {
        let Some(origin) = drag_origin_for_update.get() else {
            return;
        };
        let Some(chart) = weak_chart.upgrade() else {
            return;
        };
        let Some(plot) = PlotArea::from_size(f64::from(chart.width()), f64::from(chart.height()))
        else {
            return;
        };
        let now = Instant::now();
        let available = state_for_update
            .history
            .borrow()
            .available_range(&state_for_update.key);
        let window = panned_window(origin, available, offset_x / plot.width(), now);
        state_for_update.view_range.set(ViewRange::Manual(window));
        refresh_range_label(&state_for_update);
        chart.queue_draw();
    });

    let drag_origin_for_end = drag_origin;
    drag.connect_drag_end(move |_, _offset_x, _offset_y| {
        drag_origin_for_end.set(None);
    });
    chart.add_controller(drag);
}

fn refresh_hover_state(state: &InteractionState, chart: &DrawingArea) {
    let Some(ratio) = state.hover_ratio.get() else {
        state.hover_point.set(None);
        state.hover_label.set_text("");
        return;
    };
    let point = {
        let history = state.history.borrow();
        let window = state
            .view_range
            .get()
            .window(history.available_range(&state.key), Instant::now());
        let points = chart_points_for_view(
            &history,
            &state.key,
            window,
            chart_point_limit(f64::from(chart.width()), f64::from(chart.height())),
            &state.chart_cache,
        );
        nearest_chart_point(&points, instant_at_ratio(window, ratio))
    };
    state.hover_point.set(point);
    let label = point.map(|point| hover_label(point, state.unit));
    state.hover_label.set_text(label.as_deref().unwrap_or(""));
}

fn refresh_range_label(state: &InteractionState) {
    update_range_label(
        &state.range_label,
        &state.history,
        &state.key,
        state.view_range.get(),
    );
}

fn update_range_label(label: &Label, history: &SharedHistory, key: &SensorKey, range: ViewRange) {
    let now = Instant::now();
    let window = range.window(history.borrow().available_range(key), now);
    label.set_text(&window_label(window, now, unix_timestamp_ms()));
}

fn clear_hover(
    hover_ratio: &Rc<Cell<Option<f64>>>,
    hover_point: &Rc<Cell<Option<HistoryPoint>>>,
    label: &Label,
) {
    hover_ratio.set(None);
    hover_point.set(None);
    label.set_text("");
}

fn hover_label(point: HistoryPoint, unit: Unit) -> String {
    let value = point
        .value
        .map(|value| format_value(value, &unit))
        .unwrap_or_else(|| "Unavailable".to_owned());
    let show_milliseconds = point.expected_interval < Duration::from_secs(1);
    format!(
        "{} — {value}",
        timestamp_label(point.timestamp_ms, show_milliseconds)
    )
}

fn timestamp_label(timestamp_ms: u128, show_milliseconds: bool) -> String {
    let seconds = i64::try_from(timestamp_ms / 1_000).unwrap_or(i64::MAX);
    glib::DateTime::from_unix_local(seconds)
        .and_then(|date_time| date_time.format("%Y-%m-%d %H:%M:%S"))
        .map(|formatted| {
            if show_milliseconds {
                format!("{formatted}.{:03}", timestamp_ms % 1_000)
            } else {
                formatted.to_string()
            }
        })
        .unwrap_or_else(|_| format!("{timestamp_ms} ms since Unix epoch"))
}

fn window_label(window: ChartWindow, now: Instant, now_timestamp_ms: u128) -> String {
    let start = timestamp_ms_at(window.start(), now, now_timestamp_ms);
    let end = timestamp_ms_at(window.end, now, now_timestamp_ms);
    format!(
        "{} — {}",
        timestamp_label(start, false),
        timestamp_label(end, false)
    )
}

fn timestamp_ms_at(target: Instant, now: Instant, now_timestamp_ms: u128) -> u128 {
    if target <= now {
        now_timestamp_ms.saturating_sub(now.duration_since(target).as_millis())
    } else {
        now_timestamp_ms.saturating_add(target.duration_since(now).as_millis())
    }
}

fn unix_timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[derive(Clone)]
pub(crate) struct DurationSelector {
    pub(crate) widget: gtk::Box,
    spin: SpinButton,
    unit: ComboBoxText,
    seconds: Rc<Cell<u64>>,
    updating: Rc<Cell<bool>>,
    max_seconds: u64,
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

    fn set_display_duration(&self, duration: Duration) {
        let seconds = duration
            .as_secs()
            .clamp(MIN_HISTORY_RETENTION.as_secs(), self.max_seconds);
        self.write_controls(seconds);
        let normalized = self.read_controls();
        self.seconds.set(normalized);
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

fn chart_points_for_view(
    history: &HistoryStore,
    key: &SensorKey,
    window: ChartWindow,
    max_points: usize,
    cache: &RefCell<ChartCache>,
) -> Vec<ChartPoint> {
    let revision = history.revision();
    let mut cache = cache.borrow_mut();
    if cache.revision != revision || cache.window != Some(window) || cache.max_points != max_points
    {
        cache.revision = revision;
        cache.window = Some(window);
        cache.max_points = max_points;
        cache.points = history.chart_points(key, window.duration, window.end, max_points);
    }
    cache.points.clone()
}

fn chart_point_limit(width: f64, height: f64) -> usize {
    PlotArea::from_size(width, height)
        .map(|plot| plot.width().max(64.0) as usize)
        .unwrap_or(64)
}

fn install_draw_func(
    chart: &DrawingArea,
    history: SharedHistory,
    key: SensorKey,
    view_range: Rc<Cell<ViewRange>>,
    hover_ratio: Rc<Cell<Option<f64>>>,
    chart_cache: Rc<RefCell<ChartCache>>,
    unit: Unit,
) {
    chart.set_draw_func(move |_area, context, width, height| {
        let width = f64::from(width);
        let height = f64::from(height);
        let Some(plot) = PlotArea::from_size(width, height) else {
            return;
        };
        let plot_width = plot.width();
        let plot_height = plot.height();

        context.set_source_rgba(0.5, 0.5, 0.5, 0.20);
        context.set_line_width(1.0);
        for index in 0..=4 {
            let y = plot.top + plot_height * f64::from(index) / 4.0;
            context.move_to(plot.left, y);
            context.line_to(plot.right, y);
        }
        let _ = context.stroke();

        let max_points = chart_point_limit(width, height);
        let now = Instant::now();
        let history = history.borrow();
        let range = view_range.get();
        let window = range.window(history.available_range(&key), now);
        let points = chart_points_for_view(&history, &key, window, max_points, &chart_cache);
        let hovered = hover_ratio
            .get()
            .and_then(|ratio| nearest_chart_point(&points, instant_at_ratio(window, ratio)));
        let mut values = points.iter().filter_map(|point| point.value);
        let Some(first) = values.next() else {
            draw_empty(context, plot.left, plot.top + plot_height / 2.0);
            draw_hover(context, plot, window, hovered, None);
            draw_time_labels(context, plot, window.duration, range.end_label());
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
        for point in &points {
            let Some(value) = point.value else {
                if active_path {
                    let _ = context.stroke();
                    active_path = false;
                }
                continue;
            };
            let x = plot.left + point.x * plot_width;
            let y = plot.bottom - (value - minimum) / (maximum - minimum) * plot_height;
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

        draw_hover(context, plot, window, hovered, Some((minimum, maximum)));
        draw_value_labels(context, plot.top, plot.bottom, minimum, maximum, unit);
        draw_time_labels(context, plot, window.duration, range.end_label());
    });
}

fn draw_hover(
    context: &cairo::Context,
    plot: PlotArea,
    window: ChartWindow,
    point: Option<HistoryPoint>,
    bounds: Option<(f64, f64)>,
) {
    let Some(point) = point.filter(|point| window.contains(point.captured_at)) else {
        return;
    };
    let ratio = point
        .captured_at
        .saturating_duration_since(window.start())
        .as_secs_f64()
        / window.duration.as_secs_f64();
    let x = plot.left + ratio.clamp(0.0, 1.0) * plot.width();

    context.set_source_rgba(0.5, 0.5, 0.5, 0.70);
    context.set_line_width(1.0);
    context.move_to(x, plot.top);
    context.line_to(x, plot.bottom);
    let _ = context.stroke();

    let (Some(value), Some((minimum, maximum))) = (point.value, bounds) else {
        return;
    };
    let y = plot.bottom - (value - minimum) / (maximum - minimum) * plot.height();
    context.set_source_rgba(0.20, 0.52, 0.88, 1.0);
    context.arc(x, y, 3.5, 0.0, std::f64::consts::TAU);
    let _ = context.fill();
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

fn draw_time_labels(context: &cairo::Context, plot: PlotArea, window: Duration, end_label: &str) {
    context.set_source_rgba(0.5, 0.5, 0.5, 0.90);
    context.set_font_size(11.0);
    context.move_to(plot.left, plot.bottom + 18.0);
    let _ = context.show_text(&format!("−{}", duration_label(window)));
    context.move_to(plot.right - 40.0, plot.bottom + 18.0);
    let _ = context.show_text(end_label);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_available_uses_the_recorded_range() {
        let now = Instant::now();
        let latest = now - Duration::from_secs(5);
        let recorded = Duration::from_secs(7 * 60);
        let window = ViewRange::AllAvailable.window(Some((latest, recorded)), now);

        assert_eq!(window.duration, recorded);
        assert_eq!(window.end, latest);
    }

    #[test]
    fn zoom_keeps_the_pointer_timestamp_stationary() {
        let now = Instant::now();
        let window = ChartWindow {
            duration: Duration::from_secs(60 * 60),
            end: now - Duration::from_secs(2 * 60 * 60),
        };
        let anchor = 0.5;
        let anchored_at = instant_at_ratio(window, anchor);
        let zoomed = zoomed_window(
            window,
            Some((now, Duration::from_secs(6 * 60 * 60))),
            anchor,
            ZOOM_STEP,
            now,
        );

        assert_eq!(instant_at_ratio(zoomed, anchor), anchored_at);
        assert_eq!(zoomed.duration, Duration::from_secs(75 * 60));
    }

    #[test]
    fn timestamp_mapping_preserves_instant_offsets() {
        let now = Instant::now();
        let timestamp_ms = 10_000;

        assert_eq!(
            timestamp_ms_at(now - Duration::from_millis(1_500), now, timestamp_ms),
            8_500
        );
        assert_eq!(
            timestamp_ms_at(now + Duration::from_millis(250), now, timestamp_ms),
            10_250
        );
    }

    #[test]
    fn hover_timestamp_precision_follows_the_sample_interval() {
        let captured_at = Instant::now();
        let subsecond = HistoryPoint {
            captured_at,
            timestamp_ms: 1_700_000_000_234,
            expected_interval: Duration::from_millis(200),
            value: Some(40.8),
        };
        let one_second = HistoryPoint {
            expected_interval: Duration::from_secs(1),
            ..subsecond
        };

        assert!(hover_label(subsecond, Unit::Celsius).contains(".234"));
        assert!(!hover_label(one_second, Unit::Celsius).contains(".234"));
    }

    #[test]
    fn panning_stays_inside_the_recorded_range() {
        let now = Instant::now();
        let available = Some((now, Duration::from_secs(4 * 60 * 60)));
        let current = ChartWindow {
            duration: Duration::from_secs(60 * 60),
            end: now,
        };

        let oldest = panned_window(current, available, 10.0, now);
        assert_eq!(oldest.end, now - Duration::from_secs(3 * 60 * 60));

        let latest = panned_window(oldest, available, -10.0, now);
        assert_eq!(latest.end, now);
    }

    #[test]
    fn export_filename_uses_selected_format() {
        assert!(default_export_filename(LogFormat::Csv).ends_with(".csv"));
        assert!(default_export_filename(LogFormat::JsonLines).ends_with(".jsonl"));
    }

    #[test]
    fn export_extension_is_added_only_when_missing() {
        assert_eq!(
            ensure_export_extension(PathBuf::from("history"), LogFormat::Csv),
            PathBuf::from("history.csv")
        );
        assert_eq!(
            ensure_export_extension(PathBuf::from("history.custom"), LogFormat::Csv),
            PathBuf::from("history.custom")
        );
    }
}
