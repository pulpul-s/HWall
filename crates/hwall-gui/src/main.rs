mod alert_editor;
mod dialogs;
mod hardware;
mod history;
mod history_chart;
mod session;
mod table;
mod tray;
mod ui;
mod window;

use gtk::glib;
use gtk::prelude::*;
use gtk::{Application, ApplicationWindow, Orientation};
use history::SensorKey;
use hwall_app::{
    alert_supported_sensor, build_hardware_inventory, build_sensor_rows, default_log_directory,
    ordered_device_entries, plasma_window_placement_supported, sensor_key,
    sync_plasma_window_placement, timestamped_log_path, AlertEngine, AlertEvent, AlertSeverity,
    AppSettings, LogScope, RowKind, RowOptions, SensorRow, SettingsStore, APPLICATION_ID,
};
use hwall_core::{supports_storage_health, Device, DeviceClass, Sensor};
use session::{Activity, HealthRefreshReason, Session};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};
use tray::{TrayAction, TrayBridge};
use ui::restore_scroll_position;
use window::{ToolbarActions, Ui};

type SharedModel = Rc<RefCell<GuiModel>>;

struct GuiModel {
    settings: AppSettings,
    settings_store: SettingsStore,
    session: Session,
    alerts: AlertEngine,
    visible_sensor_count: usize,
    tray: TrayBridge,
    notice: String,
    plasma_map_sync_pending: bool,
    sensor_details: BTreeMap<SensorKey, glib::WeakRef<gtk::Window>>,
    device_details: BTreeMap<String, glib::WeakRef<gtk::Window>>,
    next_health_view_check: Instant,
    sensor_query: String,
    hardware_query: String,
}

fn main() -> glib::ExitCode {
    let application = Application::builder()
        .application_id(APPLICATION_ID)
        .build();
    application.connect_activate(activate);
    application.run()
}

fn activate(application: &Application) {
    gtk::Window::set_default_icon_name(APPLICATION_ID);
    if let Some(window) = application
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<ApplicationWindow>().ok())
    {
        window.present();
        return;
    }
    build_ui(application);
}

fn build_ui(application: &Application) {
    let settings_store = SettingsStore::discover();
    let initial = settings_store.load();
    let interval = Duration::from_millis(initial.interval_ms.max(100));
    let rediscover = Duration::from_secs(initial.rediscover_seconds.max(5));
    let health_interval = Duration::from_secs(initial.health_interval_seconds.max(60));
    let session = Session::spawn(
        interval,
        rediscover,
        health_interval,
        initial.show_identifying_information,
        initial.history_retention(),
    );
    let tray = TrayBridge::start();
    let (plasma_map_sync_pending, initial_notice) =
        synchronize_plasma_window_placement(initial.plasma_window_placement, false);

    let (ui, actions) = window::build(application, &initial);
    let model = Rc::new(RefCell::new(GuiModel {
        settings: initial.clone(),
        settings_store,
        session,
        alerts: AlertEngine::default(),
        visible_sensor_count: 0,
        tray,
        notice: initial_notice.unwrap_or_default(),
        plasma_map_sync_pending,
        sensor_details: BTreeMap::new(),
        device_details: BTreeMap::new(),
        next_health_view_check: Instant::now(),
        sensor_query: String::new(),
        hardware_query: String::new(),
    }));

    connect_actions(&model, &ui, actions);
    connect_window_lifecycle(&model, &ui);
    connect_view_switching(&model, &ui);
    connect_plasma_window_placement(&model, &ui);
    start_tick(model.clone(), ui.clone());

    if initial.start_hidden && model.borrow().tray.available {
        ui.window.hide();
    } else {
        ui.window.present();
    }
}

fn connect_actions(model: &SharedModel, ui: &Ui, actions: ToolbarActions) {
    let ToolbarActions {
        reset,
        rediscover,
        hidden,
        organize,
        settings,
    } = actions;

    let model_for_health = model.clone();
    let ui_for_health = ui.clone();
    ui.hardware
        .set_refresh_health_handler(move |device_id, elevated| {
            request_health_refresh(
                &model_for_health,
                vec![device_id],
                if elevated {
                    HealthRefreshReason::ElevatedManual
                } else {
                    HealthRefreshReason::Manual
                },
            );
            model_for_health.borrow_mut().notice = if elevated {
                "Administrator storage health refresh requested".to_owned()
            } else {
                "Storage health refresh requested".to_owned()
            };
            update_status(&model_for_health, &ui_for_health);
        });

    let model_for_search = model.clone();
    let ui_for_search = ui.clone();
    ui.search.connect_search_changed(move |entry| {
        let query = entry.text().to_string();
        if active_view_is_hardware(&ui_for_search) {
            model_for_search.borrow_mut().hardware_query = query.clone();
            ui_for_search.hardware.set_query(&query);
            update_status(&model_for_search, &ui_for_search);
        } else {
            model_for_search.borrow_mut().sensor_query = query;
            rebuild_rows(&model_for_search, &ui_for_search);
        }
    });

    let model_for_pause = model.clone();
    let ui_for_pause = ui.clone();
    ui.pause_button.connect_clicked(move |_| {
        model_for_pause.borrow_mut().session.toggle_paused();
        update_status(&model_for_pause, &ui_for_pause);
    });

    let model_for_reset = model.clone();
    let ui_for_reset = ui.clone();
    reset.connect_clicked(move |_| {
        {
            let mut borrowed = model_for_reset.borrow_mut();
            borrowed.session.reset_statistics();
            borrowed.notice = "Statistics reset".to_owned();
        }
        rebuild_rows(&model_for_reset, &ui_for_reset);
    });

    let model_for_rediscover = model.clone();
    let ui_for_rediscover = ui.clone();
    rediscover.connect_clicked(move |_| {
        {
            let mut borrowed = model_for_rediscover.borrow_mut();
            borrowed.session.rediscover();
            borrowed.notice = "Hardware rediscovery requested".to_owned();
        }
        update_status(&model_for_rediscover, &ui_for_rediscover);
    });

    let model_for_hidden = model.clone();
    let ui_for_hidden = ui.clone();
    hidden.connect_clicked(move |_| {
        show_hidden_items(&model_for_hidden, &ui_for_hidden);
    });

    let model_for_logging = model.clone();
    let ui_for_logging = ui.clone();
    ui.logging_button.connect_clicked(move |_| {
        toggle_logging(&model_for_logging, &ui_for_logging);
    });

    let model_for_favorites_only = model.clone();
    let ui_for_favorites_only = ui.clone();
    ui.favorites_button.connect_toggled(move |button| {
        model_for_favorites_only
            .borrow_mut()
            .settings
            .favorites_only = button.is_active();
        save_settings(&model_for_favorites_only, &ui_for_favorites_only);
        rebuild_rows(&model_for_favorites_only, &ui_for_favorites_only);
    });

    let model_for_organize = model.clone();
    let ui_for_organize = ui.clone();
    organize.connect_clicked(move |_| {
        let devices = {
            let borrowed = model_for_organize.borrow();
            ordered_device_entries(borrowed.session.snapshot(), &borrowed.settings.device_order)
        };
        let model_after_apply = model_for_organize.clone();
        let ui_after_apply = ui_for_organize.clone();
        dialogs::show_device_order(&ui_for_organize.window, devices, move |device_order| {
            model_after_apply.borrow_mut().settings.device_order = device_order;
            save_settings(&model_after_apply, &ui_after_apply);
            rebuild_rows(&model_after_apply, &ui_after_apply);
        });
    });

    let model_for_settings = model.clone();
    let ui_for_settings = ui.clone();
    settings.connect_clicked(move |_| {
        let current = model_for_settings.borrow().settings.clone();
        let model_after_apply = model_for_settings.clone();
        let ui_after_apply = ui_for_settings.clone();
        let plasma_placement_available = plasma_window_placement_supported();
        dialogs::show_settings(
            &ui_for_settings.window,
            current,
            ui_for_settings.table.clone(),
            plasma_placement_available,
            move |settings| {
                let interval = Duration::from_millis(settings.interval_ms.max(100));
                let history_retention = settings.history_retention();
                let density = settings.density;
                let favorites_only = settings.favorites_only;
                let placement_requested = settings.plasma_window_placement;
                let allow_rule_creation = ui_after_apply.window.is_mapped();
                let (plasma_map_sync_pending, placement_notice) =
                    synchronize_plasma_window_placement(placement_requested, allow_rule_creation);
                let identifying_information_changed = {
                    let mut borrowed = model_after_apply.borrow_mut();
                    let changed = borrowed.settings.show_identifying_information
                        != settings.show_identifying_information;
                    borrowed.settings = settings;
                    borrowed.session.set_interval(interval);
                    borrowed.session.set_history_retention(history_retention);
                    if changed {
                        let include_sensitive = borrowed.settings.show_identifying_information;
                        borrowed
                            .session
                            .set_identifying_information(include_sensitive);
                    }
                    borrowed.plasma_map_sync_pending = plasma_map_sync_pending;
                    changed
                };
                ui_after_apply.table.set_density_class(density);
                ui_after_apply.favorites_button.set_active(favorites_only);
                update_plasma_notice(&model_after_apply, placement_notice);
                if identifying_information_changed {
                    model_after_apply.borrow_mut().notice =
                        "Updating identifying information…".to_owned();
                }
                save_settings(&model_after_apply, &ui_after_apply);
                rebuild_rows(&model_after_apply, &ui_after_apply);
            },
        );
    });

    let model_for_activate = model.clone();
    let ui_for_activate = ui.clone();
    ui.table.view.connect_activate(move |_, _| {
        show_selected_details(&model_for_activate, &ui_for_activate);
    });

    let model_for_context = model.clone();
    let ui_for_context = ui.clone();
    ui.table.set_context_handler(move |row, anchor, x, y| {
        show_sensor_context_menu(&model_for_context, &ui_for_context, row, &anchor, x, y);
    });

    let key_controller = gtk::EventControllerKey::new();
    key_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    let model_for_keys = model.clone();
    let ui_for_keys = ui.clone();
    key_controller.connect_key_pressed(move |_, key, _, state| {
        if key == gtk::gdk::Key::space && toggle_selected_collapsed(&model_for_keys, &ui_for_keys) {
            return glib::Propagation::Stop;
        }
        if key == gtk::gdk::Key::Menu
            || (key == gtk::gdk::Key::F10 && state.contains(gtk::gdk::ModifierType::SHIFT_MASK))
        {
            if let Some(row) = ui_for_keys.table.selected_row() {
                let anchor: gtk::Widget = ui_for_keys.table.view.clone().upcast();
                show_sensor_context_menu(&model_for_keys, &ui_for_keys, row, &anchor, 12.0, 12.0);
            }
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    ui.table.view.add_controller(key_controller);
}

fn active_view_is_hardware(ui: &Ui) -> bool {
    ui.content_stack.visible_child_name().as_deref() == Some("hardware")
}

fn toggle_selected_collapsed(model: &SharedModel, ui: &Ui) -> bool {
    let Some(row) = ui.table.selected_row() else {
        return false;
    };
    toggle_row_collapsed(model, ui, &row)
}

fn toggle_row_collapsed(model: &SharedModel, ui: &Ui, row: &SensorRow) -> bool {
    if !matches!(row.kind, RowKind::Device | RowKind::Header) {
        return false;
    }
    model
        .borrow_mut()
        .settings
        .visibility
        .toggle_collapsed(row.hide_key.clone());
    save_settings(model, ui);
    rebuild_rows(model, ui);
    true
}

fn toggle_row_favorite(model: &SharedModel, ui: &Ui, row: &SensorRow) {
    if row.kind != RowKind::Sensor {
        return;
    }
    model
        .borrow_mut()
        .settings
        .visibility
        .toggle_favorite(row.hide_key.clone());
    save_settings(model, ui);
    rebuild_rows(model, ui);
}

fn rename_sensor_row(model: &SharedModel, ui: &Ui, row: SensorRow) {
    if row.kind != RowKind::Sensor {
        return;
    }
    let key = row.hide_key.clone();
    let original_label = row.original_label.clone();
    let model_after_apply = model.clone();
    let ui_after_apply = ui.clone();
    dialogs::show_sensor_alias(&ui.window, row, move |alias| {
        let mut borrowed = model_after_apply.borrow_mut();
        match alias.map(|value| value.trim().to_owned()) {
            Some(value) if !value.is_empty() && value != original_label => {
                borrowed.settings.sensor_aliases.insert(key.clone(), value);
            }
            _ => {
                borrowed.settings.sensor_aliases.remove(&key);
            }
        }
        drop(borrowed);
        save_settings(&model_after_apply, &ui_after_apply);
        rebuild_rows(&model_after_apply, &ui_after_apply);
    });
}

fn hide_sensor_row(model: &SharedModel, ui: &Ui, row: &SensorRow) {
    model
        .borrow_mut()
        .settings
        .visibility
        .hide(row.hide_key.clone(), row.label.clone());
    save_settings(model, ui);
    rebuild_rows(model, ui);
}

fn alert_capable_sensor(model: &SharedModel, row: &SensorRow) -> bool {
    sensor_for_row(model, row)
        .is_some_and(|sensor| sensor.value.is_some() && alert_supported_sensor(&sensor))
}

fn sensor_for_row(model: &SharedModel, row: &SensorRow) -> Option<Sensor> {
    let sensor_id = row.sensor_id.as_deref()?;
    model
        .borrow()
        .session
        .snapshot()
        .devices
        .iter()
        .find(|device| device.id == row.device_id)?
        .sensors
        .iter()
        .find(|sensor| sensor.id == sensor_id)
        .cloned()
}

fn configure_sensor_alert(model: &SharedModel, ui: &Ui, parent: &gtk::Window, row: SensorRow) {
    let Some(sensor) = sensor_for_row(model, &row) else {
        return;
    };
    let key = row.hide_key.clone();
    let label = row.label;
    let notice_label = label.clone();
    let current = model.borrow().settings.sensor_alerts.get(&key).cloned();
    let model_after_apply = model.clone();
    let ui_after_apply = ui.clone();
    alert_editor::show(
        parent,
        &label,
        sensor.kind,
        sensor.unit,
        current,
        move |rule| {
            {
                let mut borrowed = model_after_apply.borrow_mut();
                match rule {
                    Some(rule) => {
                        borrowed.settings.sensor_alerts.insert(key.clone(), rule);
                        borrowed.notice = format!("Alert configured for {notice_label}");
                    }
                    None => {
                        borrowed.settings.sensor_alerts.remove(&key);
                        borrowed.notice = format!("Alert disabled for {notice_label}");
                    }
                }
                borrowed.alerts.reset(&key);
            }
            ui_after_apply
                .application
                .withdraw_notification(&format!("hwall-alert:{key}"));
            save_settings(&model_after_apply, &ui_after_apply);
            rebuild_rows(&model_after_apply, &ui_after_apply);
        },
    );
}

fn show_sensor_context_menu(
    model: &SharedModel,
    ui: &Ui,
    row: SensorRow,
    anchor: &gtk::Widget,
    x: f64,
    y: f64,
) {
    let popover = gtk::Popover::new();
    popover.add_css_class("menu");
    popover.set_has_arrow(true);
    popover.set_parent(anchor);
    popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(
        x.max(0.0) as i32,
        y.max(0.0) as i32,
        1,
        1,
    )));

    let menu = gtk::Box::new(Orientation::Vertical, 0);
    menu.set_margin_top(4);
    menu.set_margin_bottom(4);
    menu.set_margin_start(4);
    menu.set_margin_end(4);

    let model_for_details = model.clone();
    let ui_for_details = ui.clone();
    let row_for_details = row.clone();
    append_context_action(&menu, &popover, "View details", move || {
        show_details_for_row(&model_for_details, &ui_for_details, row_for_details.clone());
    });

    if row.kind == RowKind::Sensor && alert_capable_sensor(model, &row) {
        let model_for_alert = model.clone();
        let ui_for_alert = ui.clone();
        let row_for_alert = row.clone();
        let parent: gtk::Window = ui.window.clone().upcast();
        append_context_action(&menu, &popover, "Configure alert…", move || {
            configure_sensor_alert(
                &model_for_alert,
                &ui_for_alert,
                &parent,
                row_for_alert.clone(),
            );
        });
    }

    if matches!(row.kind, RowKind::Device | RowKind::Header) {
        let model_for_collapse = model.clone();
        let ui_for_collapse = ui.clone();
        let row_for_collapse = row.clone();
        append_context_action(
            &menu,
            &popover,
            if row.collapsed { "Expand" } else { "Collapse" },
            move || {
                toggle_row_collapsed(&model_for_collapse, &ui_for_collapse, &row_for_collapse);
            },
        );
    }

    if row.kind == RowKind::Sensor {
        let model_for_favorite = model.clone();
        let ui_for_favorite = ui.clone();
        let row_for_favorite = row.clone();
        append_context_action(
            &menu,
            &popover,
            if row.favorite {
                "Remove from favorites"
            } else {
                "Add to favorites"
            },
            move || toggle_row_favorite(&model_for_favorite, &ui_for_favorite, &row_for_favorite),
        );

        let model_for_rename = model.clone();
        let ui_for_rename = ui.clone();
        let row_for_rename = row.clone();
        append_context_action(&menu, &popover, "Rename", move || {
            rename_sensor_row(&model_for_rename, &ui_for_rename, row_for_rename.clone());
        });
    }

    let model_for_hide = model.clone();
    let ui_for_hide = ui.clone();
    let row_for_hide = row;
    append_context_action(&menu, &popover, "Hide", move || {
        hide_sensor_row(&model_for_hide, &ui_for_hide, &row_for_hide);
    });

    popover.set_child(Some(&menu));
    popover.connect_closed(|popover| popover.unparent());
    popover.popup();
}

fn append_context_action(
    menu: &gtk::Box,
    popover: &gtk::Popover,
    label: &str,
    action: impl Fn() + 'static,
) {
    let button = gtk::Button::with_label(label);
    button.add_css_class("flat");
    button.set_halign(gtk::Align::Fill);
    button.set_hexpand(true);
    if let Some(label) = button.child().and_downcast::<gtk::Label>() {
        label.set_xalign(0.0);
        label.set_halign(gtk::Align::Fill);
    }
    let popover = popover.clone();
    button.connect_clicked(move |_| {
        popover.popdown();
        action();
    });
    menu.append(&button);
}

fn show_selected_details(model: &SharedModel, ui: &Ui) {
    let Some(row) = ui.table.selected_row() else {
        model.borrow_mut().notice = "Select a device or sensor to inspect".to_owned();
        update_status(model, ui);
        return;
    };
    show_details_for_row(model, ui, row);
}

fn show_details_for_row(model: &SharedModel, ui: &Ui, row: SensorRow) {
    let details = {
        let borrowed = model.borrow();
        let device = borrowed
            .session
            .snapshot()
            .devices
            .iter()
            .find(|device| device.id == row.device_id)
            .cloned();
        let sensor = device.as_ref().and_then(|device| {
            row.sensor_id.as_deref().and_then(|sensor_id| {
                device
                    .sensors
                    .iter()
                    .find(|sensor| sensor.id == sensor_id)
                    .cloned()
            })
        });
        let observed = row.sensor_id.as_deref().and_then(|sensor_id| {
            borrowed
                .session
                .statistics()
                .get(&row.device_id, sensor_id)
                .copied()
        });
        let alert_rule = borrowed.settings.sensor_alerts.get(&row.hide_key).cloned();
        let alert_state = borrowed.alerts.state(&row.hide_key);
        (
            device,
            sensor,
            observed,
            alert_rule,
            alert_state,
            borrowed.session.history(),
            borrowed.settings.logging_format,
        )
    };

    match details {
        (Some(device), Some(sensor), observed, alert_rule, alert_state, history, log_format) => {
            let key = SensorKey::new(&device.id, &sensor.id);
            let existing = {
                let mut borrowed = model.borrow_mut();
                borrowed
                    .sensor_details
                    .retain(|_, weak| weak.upgrade().is_some());
                borrowed
                    .sensor_details
                    .get(&key)
                    .and_then(|weak| weak.upgrade())
            };
            if let Some(window) = existing {
                window.present();
                return;
            }

            let model_for_live = model.clone();
            let key_for_live = key.clone();
            let read_live = move || {
                let borrowed = model_for_live.borrow();
                let device = borrowed
                    .session
                    .snapshot()
                    .devices
                    .iter()
                    .find(|device| device.id.as_str() == key_for_live.device_id.as_str())?
                    .clone();
                let sensor = device
                    .sensors
                    .iter()
                    .find(|sensor| sensor.id.as_str() == key_for_live.sensor_id.as_str())?
                    .clone();
                let observed = borrowed
                    .session
                    .statistics()
                    .get(&key_for_live.device_id, &key_for_live.sensor_id)
                    .copied();
                let alert_key = sensor_key(&key_for_live.device_id, &key_for_live.sensor_id);
                let alert_rule = borrowed.settings.sensor_alerts.get(&alert_key).cloned();
                let alert_state = borrowed.alerts.state(&alert_key);
                Some((device, sensor, observed, alert_rule, alert_state))
            };

            let model_for_history = model.clone();
            let ui_for_history = ui.clone();
            let model_for_export = model.clone();
            let ui_for_export = ui.clone();
            let model_for_alert = model.clone();
            let ui_for_alert = ui.clone();
            let row_for_alert = row.clone();
            let window = dialogs::show_sensor_details(
                &ui.window,
                dialogs::SensorDetailsRequest {
                    device,
                    sensor,
                    observed,
                    alert_rule,
                    alert_state,
                    row,
                    history,
                    history_key: key.clone(),
                    default_log_format: log_format,
                    read_live: Box::new(read_live),
                    configure_alert: Box::new(move |parent| {
                        configure_sensor_alert(
                            &model_for_alert,
                            &ui_for_alert,
                            parent,
                            row_for_alert.clone(),
                        );
                    }),
                    recording_changed: Rc::new(move || {
                        update_status(&model_for_history, &ui_for_history)
                    }),
                    exported: Box::new(move |result| {
                        let mut borrowed = model_for_export.borrow_mut();
                        borrowed.notice = match result {
                            Ok(path) => format!("History exported to {}", path.display()),
                            Err(error) => format!("History export failed: {error}"),
                        };
                        drop(borrowed);
                        update_status(&model_for_export, &ui_for_export);
                    }),
                },
            );
            model
                .borrow_mut()
                .sensor_details
                .insert(key.clone(), window.downgrade());

            let model_for_close = model.clone();
            let ui_for_close = ui.clone();
            window.connect_close_request(move |_| {
                model_for_close.borrow_mut().sensor_details.remove(&key);
                update_status(&model_for_close, &ui_for_close);
                glib::Propagation::Proceed
            });
            update_status(model, ui);
        }
        (Some(device), None, _, _, _, _, _) => show_device_details(model, ui, device),
        _ => {
            model.borrow_mut().notice = "The selected item is no longer available".to_owned();
            update_status(model, ui);
        }
    }
}

fn show_device_details(model: &SharedModel, ui: &Ui, device: Device) {
    let existing = {
        let mut borrowed = model.borrow_mut();
        borrowed
            .device_details
            .retain(|_, weak| weak.upgrade().is_some());
        borrowed
            .device_details
            .get(&device.id)
            .and_then(|weak| weak.upgrade())
    };
    if let Some(window) = existing {
        window.present();
        return;
    }

    if device.class == DeviceClass::Storage {
        request_stale_health(model, vec![device.id.clone()]);
    }
    let device_id = device.id.clone();
    let model_for_live = model.clone();
    let device_id_for_live = device_id.clone();
    let read_live = move || {
        model_for_live
            .borrow()
            .session
            .snapshot()
            .devices
            .iter()
            .find(|candidate| candidate.id == device_id_for_live)
            .cloned()
    };
    let model_for_refresh = model.clone();
    let ui_for_refresh = ui.clone();
    let refreshable = supports_storage_health(&device);
    let window = dialogs::show_device_details(
        &ui.window,
        device,
        refreshable,
        read_live,
        move |device_id, elevated| {
            request_health_refresh(
                &model_for_refresh,
                vec![device_id],
                if elevated {
                    HealthRefreshReason::ElevatedManual
                } else {
                    HealthRefreshReason::Manual
                },
            );
            model_for_refresh.borrow_mut().notice = if elevated {
                "Administrator storage health refresh requested".to_owned()
            } else {
                "Storage health refresh requested".to_owned()
            };
            update_status(&model_for_refresh, &ui_for_refresh);
        },
    );
    model
        .borrow_mut()
        .device_details
        .insert(device_id.clone(), window.downgrade());
    let model_for_close = model.clone();
    window.connect_close_request(move |_| {
        model_for_close
            .borrow_mut()
            .device_details
            .remove(&device_id);
        glib::Propagation::Proceed
    });
}

fn request_health_refresh(
    model: &SharedModel,
    device_ids: Vec<String>,
    reason: HealthRefreshReason,
) {
    model
        .borrow_mut()
        .session
        .refresh_storage_health(device_ids, reason);
}

fn request_stale_health(model: &SharedModel, candidate_ids: Vec<String>) {
    let stale = {
        let borrowed = model.borrow();
        let maximum_age_ms = u128::from(borrowed.settings.health_interval_seconds.max(60)) * 1_000;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        borrowed
            .session
            .snapshot()
            .devices
            .iter()
            .filter(|device| candidate_ids.iter().any(|id| id == &device.id))
            .filter(|device| supports_storage_health(device))
            .filter(|device| {
                device
                    .storage_health
                    .as_ref()
                    .is_none_or(|health| health.needs_refresh(now, maximum_age_ms))
            })
            .map(|device| device.id.clone())
            .collect::<Vec<_>>()
    };
    request_health_refresh(model, stale, HealthRefreshReason::View);
}

fn request_visible_storage_health(model: &SharedModel, ui: &Ui, force: bool) {
    let should_check = {
        let mut borrowed = model.borrow_mut();
        let now = Instant::now();
        if !force && now < borrowed.next_health_view_check {
            false
        } else {
            borrowed.next_health_view_check = now + Duration::from_secs(5);
            true
        }
    };
    if !should_check {
        return;
    }
    let mut ids = Vec::new();
    if ui.content_stack.visible_child_name().as_deref() == Some("hardware") {
        ids.extend(
            model
                .borrow()
                .session
                .snapshot()
                .devices
                .iter()
                .filter(|device| supports_storage_health(device))
                .map(|device| device.id.clone()),
        );
    }
    {
        let mut borrowed = model.borrow_mut();
        borrowed
            .device_details
            .retain(|_, weak| weak.upgrade().is_some());
        ids.extend(borrowed.device_details.keys().cloned());
    }
    if !ids.is_empty() {
        request_stale_health(model, ids);
    }
}

fn connect_view_switching(model: &SharedModel, ui: &Ui) {
    let model_for_switch = model.clone();
    let ui_for_switch = ui.clone();
    ui.content_stack
        .connect_visible_child_name_notify(move |_| {
            let hardware = active_view_is_hardware(&ui_for_switch);
            ui_for_switch.sensor_controls.set_visible(!hardware);
            ui_for_switch.search.set_placeholder_text(Some(if hardware {
                "Search hardware"
            } else {
                "Filter sensors"
            }));
            let query = {
                let borrowed = model_for_switch.borrow();
                if hardware {
                    borrowed.hardware_query.clone()
                } else {
                    borrowed.sensor_query.clone()
                }
            };
            let query_changed = ui_for_switch.search.text().as_str() != query.as_str();
            if query_changed {
                ui_for_switch.search.set_text(&query);
            }
            if hardware {
                ui_for_switch.hardware.set_query(&query);
                sync_hardware(&model_for_switch, &ui_for_switch);
                request_visible_storage_health(&model_for_switch, &ui_for_switch, true);
            } else if !query_changed {
                rebuild_rows(&model_for_switch, &ui_for_switch);
            }
            update_status(&model_for_switch, &ui_for_switch);
        });
}

fn sync_hardware(model: &SharedModel, ui: &Ui) {
    let inventory = {
        let borrowed = model.borrow();
        let alert_states = borrowed.alerts.states();
        build_hardware_inventory(
            borrowed.session.snapshot(),
            borrowed.session.statistics(),
            &borrowed.settings.sensor_aliases,
            &borrowed.settings.sensor_alerts,
            &alert_states,
        )
    };
    ui.hardware.sync(inventory);
}

fn connect_window_lifecycle(model: &SharedModel, ui: &Ui) {
    let model_for_close = model.clone();
    let ui_for_close = ui.clone();
    ui.window.connect_close_request(move |window| {
        let (close_to_tray, tray_available) = {
            let borrowed = model_for_close.borrow();
            (borrowed.settings.close_to_tray, borrowed.tray.available)
        };
        save_settings(&model_for_close, &ui_for_close);
        if close_to_tray && tray_available {
            window.hide();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
}

fn connect_plasma_window_placement(model: &SharedModel, ui: &Ui) {
    let model_for_map = model.clone();
    let ui_for_map = ui.clone();
    ui.window.connect_map(move |_| {
        let placement_enabled = {
            let mut borrowed = model_for_map.borrow_mut();
            if !borrowed.plasma_map_sync_pending {
                return;
            }
            borrowed.plasma_map_sync_pending = false;
            borrowed.settings.plasma_window_placement
        };
        if !placement_enabled {
            return;
        }

        let (_, placement_notice) = synchronize_plasma_window_placement(true, true);
        update_plasma_notice(&model_for_map, placement_notice);
        update_status(&model_for_map, &ui_for_map);
    });
}

fn start_tick(model: SharedModel, ui: Ui) {
    glib::timeout_add_local(Duration::from_millis(16), move || {
        let tray_actions: Vec<_> = {
            let borrowed = model.borrow();
            borrowed.tray.actions.try_iter().collect()
        };
        for action in tray_actions {
            if handle_tray_action(action, &model, &ui) {
                return glib::ControlFlow::Break;
            }
        }

        let session::TickResult {
            snapshot_changed,
            telemetry_sample_changed,
            activity_changed,
            logging_error,
            storage_health_changed,
        } = model.borrow_mut().session.tick();
        let alert_events = if telemetry_sample_changed {
            evaluate_alerts(&model)
        } else {
            Vec::new()
        };
        if snapshot_changed {
            let identifying_information_pending =
                model.borrow().session.identifying_information_pending();
            if !identifying_information_pending {
                model.borrow_mut().notice.clear();
            }
            rebuild_rows(&model, &ui);
            if telemetry_sample_changed {
                write_log_sample(&model);
            }
        }
        send_alert_notifications(&ui, alert_events);
        if let Some(error) = logging_error.as_deref() {
            let mut borrowed = model.borrow_mut();
            borrowed.notice = format!("Logging error: {error}");
            borrowed.session.stop_logging();
        }
        if storage_health_changed {
            sync_hardware(&model, &ui);
        }
        request_visible_storage_health(&model, &ui, false);
        if (!snapshot_changed && activity_changed) || logging_error.is_some() {
            update_status(&model, &ui);
        }

        glib::ControlFlow::Continue
    });
}

fn evaluate_alerts(model: &SharedModel) -> Vec<AlertEvent> {
    let mut borrowed = model.borrow_mut();
    let GuiModel {
        session,
        settings,
        alerts,
        ..
    } = &mut *borrowed;
    let mut events = alerts.evaluate(session.snapshot(), &settings.sensor_alerts, Instant::now());
    for event in &mut events {
        if let Some(alias) = settings
            .sensor_aliases
            .get(&event.sensor_key)
            .filter(|alias| !alias.trim().is_empty())
        {
            event.sensor_name.clone_from(alias);
        }
    }
    events
}

fn send_alert_notifications(ui: &Ui, events: Vec<AlertEvent>) {
    for event in events {
        let notification_id = format!("hwall-alert:{}", event.sensor_key);
        ui.application.withdraw_notification(&notification_id);
        if !event.notify {
            continue;
        }
        let (title, body) = if event.recovered {
            let state = match event.severity {
                AlertSeverity::Normal => "returned to normal",
                AlertSeverity::Warning => "dropped to warning",
                AlertSeverity::Critical => continue,
            };
            (
                format!("{} {state}", event.sensor_name),
                format!("{} — {}", event.device_name, event.value),
            )
        } else {
            let severity = match event.severity {
                AlertSeverity::Warning => "warning",
                AlertSeverity::Critical => "critical",
                AlertSeverity::Normal => continue,
            };
            (
                format!("{} {severity}", event.sensor_name),
                format!("{} — {}", event.device_name, event.value),
            )
        };
        let notification = gtk::gio::Notification::new(&title);
        notification.set_body(Some(&body));
        ui.application
            .send_notification(Some(&notification_id), &notification);
    }
}

fn handle_tray_action(action: TrayAction, model: &SharedModel, ui: &Ui) -> bool {
    match action {
        TrayAction::Show => ui.window.present(),
        TrayAction::TogglePause => {
            model.borrow_mut().session.toggle_paused();
            update_status(model, ui);
        }
        TrayAction::ToggleLogging => toggle_logging(model, ui),
        TrayAction::ResetStatistics => {
            model.borrow_mut().session.reset_statistics();
            rebuild_rows(model, ui);
        }
        TrayAction::Quit => {
            save_settings(model, ui);
            ui.application.quit();
            return true;
        }
    }
    false
}

fn rebuild_rows(model: &SharedModel, ui: &Ui) {
    let preserve = ui.table.selected_row().map(|row| row.id);
    let scroll_position = ui.sensor_scroller.vadjustment().value();
    let rows = {
        let borrowed = model.borrow();
        let alert_states = borrowed.alerts.states();
        build_sensor_rows(
            borrowed.session.snapshot(),
            borrowed.session.statistics(),
            RowOptions {
                visibility: &borrowed.settings.visibility,
                sensor_aliases: &borrowed.settings.sensor_aliases,
                device_order: &borrowed.settings.device_order,
                show_sensor_groups: borrowed.settings.show_sensor_groups,
                query: &borrowed.sensor_query,
                favorites_only: borrowed.settings.favorites_only,
                alert_rules: &borrowed.settings.sensor_alerts,
                alert_states: &alert_states,
            },
        )
    };
    let visible_sensor_count = rows
        .iter()
        .filter(|row| row.kind == RowKind::Sensor)
        .count();
    if ui.table.sync_rows(&rows, preserve.as_deref()) {
        restore_scroll_position(&ui.sensor_scroller, scroll_position);
    }
    model.borrow_mut().visible_sensor_count = visible_sensor_count;
    sync_hardware(model, ui);
    update_status(model, ui);
}

fn update_status(model: &SharedModel, ui: &Ui) {
    let borrowed = model.borrow();
    let activity = borrowed.session.activity();
    let activity_text = match activity {
        Activity::Discovering => "Discovering",
        Activity::Live => "Live",
        Activity::Paused => "Paused",
        Activity::Disconnected => "Disconnected",
    };
    ui.status_left.set_text(activity_text);
    let paused = borrowed.session.is_paused();
    let logging_active = borrowed.session.logging();
    let logging = borrowed
        .session
        .log_path()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .map(|name| format!("  •  logging {name}"))
        .unwrap_or_default();
    let history_count = borrowed.session.extended_history_count();
    let history = match history_count {
        0 => String::new(),
        1 => "  •  1 history recording".to_owned(),
        count => format!("  •  {count} history recordings"),
    };
    let note = borrowed.notice.trim();
    let notice = if note.is_empty() {
        String::new()
    } else {
        format!("  •  {note}")
    };
    let primary_count = if ui.content_stack.visible_child_name().as_deref() == Some("hardware") {
        format!("{} devices", ui.hardware.device_count())
    } else {
        format!("{} sensors", borrowed.visible_sensor_count)
    };
    ui.status_right.set_text(&format!(
        "{primary_count}  •  {} samples{logging}{history}{notice}",
        borrowed.session.sample_rounds(),
    ));
    ui.pause_button.set_icon_name(if paused {
        "media-playback-start-symbolic"
    } else {
        "media-playback-pause-symbolic"
    });
    ui.pause_button.set_tooltip_text(Some(if paused {
        "Resume updates"
    } else {
        "Pause updates"
    }));
    ui.logging_button.set_icon_name(if logging_active {
        "media-playback-stop-symbolic"
    } else {
        "media-record-symbolic"
    });
    ui.logging_button.set_tooltip_text(Some(if logging_active {
        "Stop logging"
    } else {
        "Start logging"
    }));
}

fn toggle_logging(model: &SharedModel, ui: &Ui) {
    if model.borrow().session.logging() {
        {
            let mut borrowed = model.borrow_mut();
            borrowed.session.stop_logging();
            borrowed.notice = "Logging stopped".to_owned();
        }
        update_status(model, ui);
        return;
    }

    let (format, directory) = {
        let borrowed = model.borrow();
        (
            borrowed.settings.logging_format,
            borrowed
                .settings
                .logging_directory
                .clone()
                .unwrap_or_else(default_log_directory),
        )
    };
    let path = timestamped_log_path(directory, format);
    let start_result = model
        .borrow_mut()
        .session
        .start_logging(path.clone(), format);
    match start_result {
        Ok(()) => {
            model.borrow_mut().notice = format!("Logging to {}", path.display());
            write_log_sample(model);
        }
        Err(error) => {
            model.borrow_mut().notice = format!("Could not start logging: {error}");
        }
    }
    update_status(model, ui);
}

fn write_log_sample(model: &SharedModel) {
    if !model.borrow().session.logging() {
        return;
    }
    let rows = logging_rows(model);
    if !model.borrow().session.log_rows(rows) {
        model.borrow_mut().notice = "Logger is busy; one sample was dropped".to_owned();
    }
}

fn logging_rows(model: &SharedModel) -> Vec<SensorRow> {
    let borrowed = model.borrow();
    let mut visibility = borrowed.settings.visibility.clone();
    visibility.expand_all();
    let favorites_only = match borrowed.settings.logging_scope {
        LogScope::All => {
            visibility.show_all();
            false
        }
        LogScope::Visible => false,
        LogScope::Favorites => {
            visibility.show_all();
            true
        }
    };
    let alert_states = borrowed.alerts.states();
    build_sensor_rows(
        borrowed.session.snapshot(),
        borrowed.session.statistics(),
        RowOptions {
            visibility: &visibility,
            sensor_aliases: &borrowed.settings.sensor_aliases,
            device_order: &borrowed.settings.device_order,
            show_sensor_groups: borrowed.settings.show_sensor_groups,
            query: "",
            favorites_only,
            alert_rules: &borrowed.settings.sensor_alerts,
            alert_states: &alert_states,
        },
    )
}

fn show_hidden_items(model: &SharedModel, ui: &Ui) {
    let hidden = model
        .borrow()
        .settings
        .visibility
        .hidden_items()
        .map(|(key, label)| (key.to_owned(), label.to_owned()))
        .collect();

    let model_for_show = model.clone();
    let ui_for_show = ui.clone();
    let model_for_all = model.clone();
    let ui_for_all = ui.clone();
    dialogs::show_hidden_items(
        &ui.window,
        hidden,
        move |key| {
            model_for_show.borrow_mut().settings.visibility.show(&key);
            save_settings(&model_for_show, &ui_for_show);
            rebuild_rows(&model_for_show, &ui_for_show);
        },
        move || {
            model_for_all.borrow_mut().settings.visibility.show_all();
            save_settings(&model_for_all, &ui_for_all);
            rebuild_rows(&model_for_all, &ui_for_all);
        },
    );
}

fn save_settings(model: &SharedModel, ui: &Ui) {
    let result = {
        let mut borrowed = model.borrow_mut();
        if ui.window.is_mapped() {
            if !ui.window.is_maximized() {
                borrowed.settings.window_width = ui.window.width().max(720);
                borrowed.settings.window_height = ui.window.height().max(480);
            }
            borrowed.settings.maximized = ui.window.is_maximized();
        }
        borrowed.settings.columns = ui.table.capture_columns();
        borrowed.settings_store.save(&borrowed.settings)
    };
    if let Err(error) = result {
        model.borrow_mut().notice = format!("Could not save settings: {error}");
    }
}

fn synchronize_plasma_window_placement(
    requested: bool,
    allow_rule_creation: bool,
) -> (bool, Option<String>) {
    match sync_plasma_window_placement(requested, allow_rule_creation) {
        Ok(creation_pending) => (creation_pending, None),
        Err(error) => (
            false,
            Some(format!("Could not update Plasma placement: {error}")),
        ),
    }
}

fn update_plasma_notice(model: &SharedModel, notice: Option<String>) {
    let mut borrowed = model.borrow_mut();
    if let Some(notice) = notice {
        borrowed.notice = notice;
    } else if borrowed
        .notice
        .starts_with("Could not update Plasma placement:")
    {
        borrowed.notice.clear();
    }
}
