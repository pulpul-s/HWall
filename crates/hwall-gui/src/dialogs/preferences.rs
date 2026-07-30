use super::*;
use crate::history::MAX_HISTORY_RETENTION;
use crate::history_chart::DurationSelector;
use hwall_app::MIN_REFRESH_INTERVAL_MS;

pub(crate) fn show_sensor_alias(
    parent: &ApplicationWindow,
    row: SensorRow,
    on_apply: impl Fn(Option<String>) + 'static,
) {
    let dialog = Dialog::builder()
        .title("Rename sensor")
        .transient_for(parent)
        .modal(true)
        .use_header_bar(1)
        .default_width(460)
        .build();
    dialog.add_button("Cancel", ResponseType::Cancel);
    dialog.add_button("Reset name", ResponseType::Other(1));
    dialog.add_button("Apply", ResponseType::Apply);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    body.set_margin_top(14);
    body.set_margin_bottom(14);
    body.set_margin_start(14);
    body.set_margin_end(14);

    let original = Label::new(Some(&format!("Original name: {}", row.original_label)));
    original.set_xalign(0.0);
    original.add_css_class("dim-label");
    let entry = gtk::Entry::builder()
        .placeholder_text(row.original_label.as_str())
        .activates_default(true)
        .build();
    if row.label != row.original_label {
        entry.set_text(&row.label);
    }
    body.append(&original);
    body.append(&entry);
    dialog.content_area().append(&body);
    dialog.set_default_response(ResponseType::Apply);

    dialog.connect_response(move |dialog, response| {
        match response {
            ResponseType::Apply => {
                let value = entry.text().trim().to_owned();
                on_apply((!value.is_empty()).then_some(value));
            }
            ResponseType::Other(1) => on_apply(None),
            _ => {}
        }
        dialog.close();
    });
    dialog.present();
}

pub(crate) fn show_settings(
    parent: &ApplicationWindow,
    current: AppSettings,
    table: SensorTable,
    plasma_placement_available: bool,
    on_apply: impl Fn(AppSettings) + 'static,
) {
    let dialog = Dialog::builder()
        .title("HWall Settings")
        .transient_for(parent)
        .modal(true)
        .use_header_bar(1)
        .default_width(500)
        .build();
    dialog.add_button("Cancel", ResponseType::Cancel);
    dialog.add_button("Apply", ResponseType::Apply);

    let grid = Grid::builder()
        .column_spacing(16)
        .row_spacing(10)
        .margin_top(14)
        .margin_bottom(14)
        .margin_start(14)
        .margin_end(14)
        .build();
    let interval = gtk::SpinButton::with_range(MIN_REFRESH_INTERVAL_MS as f64, 60_000.0, 100.0);
    interval.set_value(current.interval_ms as f64);
    interval.set_tooltip_text(Some("Hardware refresh interval in milliseconds"));
    attach_labeled(&grid, 0, "Refresh interval (ms)", &interval);

    let density = ComboBoxText::new();
    for value in Density::ALL {
        density.append(Some(value.id()), value.display_name());
    }
    density.set_active_id(Some(current.density.id()));
    attach_labeled(&grid, 1, "Table density", &density);

    let log_format = ComboBoxText::new();
    for format in LogFormat::ALL {
        log_format.append(Some(format.id()), format.display_name());
    }
    log_format.set_active_id(Some(current.logging_format.id()));
    attach_labeled(&grid, 2, "Logging format", &log_format);

    let log_scope = ComboBoxText::new();
    for scope in LogScope::ALL {
        log_scope.append(Some(scope.id()), scope.display_name());
    }
    log_scope.set_active_id(Some(current.logging_scope.id()));
    attach_labeled(&grid, 3, "Logging scope", &log_scope);

    let close_to_tray = CheckButton::with_label("Close the window to the system tray");
    close_to_tray.set_active(current.close_to_tray);
    grid.attach(&close_to_tray, 0, 4, 2, 1);

    let start_hidden = CheckButton::with_label("Start hidden when a system tray is available");
    start_hidden.set_active(current.start_hidden);
    grid.attach(&start_hidden, 0, 5, 2, 1);

    let plasma_window_placement =
        CheckButton::with_label("Remember window placement on KDE Plasma");
    plasma_window_placement.set_active(current.plasma_window_placement);
    plasma_window_placement.set_sensitive(plasma_placement_available);
    plasma_window_placement.set_tooltip_text(Some(if plasma_placement_available {
        "Maintains a KWin Remember rule for the compositor-owned position. HWall continues to store size and maximized state; close-to-tray uses normal hide/show."
    } else {
        "Available in a KDE Plasma session when the Plasma 6 KConfig and QDBus tools are installed."
    }));
    grid.attach(&plasma_window_placement, 0, 6, 2, 1);

    let favorites_only = CheckButton::with_label("Start in favorites-only view");
    favorites_only.set_active(current.favorites_only);
    grid.attach(&favorites_only, 0, 7, 2, 1);

    let show_sensor_groups = CheckButton::with_label("Show sensor-type subheaders");
    show_sensor_groups.set_active(current.show_sensor_groups);
    show_sensor_groups.set_tooltip_text(Some(
        "When disabled, readings are shown directly under each device",
    ));
    grid.attach(&show_sensor_groups, 0, 8, 2, 1);

    let show_identifying_information = CheckButton::with_label("Show identifying information");
    show_identifying_information.set_active(current.show_identifying_information);
    show_identifying_information.set_tooltip_text(Some(concat!(
        "Include available serial numbers, UUIDs, WWNs, MAC addresses and other ",
        "hardware identifiers. Some values may require additional permissions."
    )));
    grid.attach(&show_identifying_information, 0, 9, 2, 1);

    let history_retention = DurationSelector::new(
        "",
        current.history_retention(),
        MAX_HISTORY_RETENTION,
        |_| {},
    );
    history_retention.widget.set_tooltip_text(Some(concat!(
        "Retain this amount of recent history for every numeric sensor. ",
        "Longer periods use more memory."
    )));
    attach_labeled(&grid, 10, "Keep sensor history", &history_retention.widget);

    let columns_title = Label::new(Some("Visible columns"));
    columns_title.set_halign(Align::Start);
    columns_title.add_css_class("heading");
    grid.attach(&columns_title, 0, 11, 2, 1);

    let current_column = CheckButton::with_label("Current");
    current_column.set_active(table.column_visible("current"));
    let minimum_column = CheckButton::with_label("Minimum");
    minimum_column.set_active(table.column_visible("minimum"));
    let maximum_column = CheckButton::with_label("Maximum");
    maximum_column.set_active(table.column_visible("maximum"));
    let average_column = CheckButton::with_label("Average");
    average_column.set_active(table.column_visible("average"));
    let status_column = CheckButton::with_label("Status");
    status_column.set_active(table.column_visible("status"));

    let columns = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    for check in [
        &current_column,
        &minimum_column,
        &maximum_column,
        &average_column,
        &status_column,
    ] {
        columns.append(check);
    }
    grid.attach(&columns, 0, 12, 2, 1);

    dialog.content_area().append(&grid);
    let table_for_response = table.clone();
    dialog.connect_response(move |dialog, response| {
        if response == ResponseType::Apply {
            let mut updated = current.clone();
            updated.interval_ms =
                interval.value().round().max(MIN_REFRESH_INTERVAL_MS as f64) as u64;
            updated.density = density
                .active_id()
                .as_deref()
                .and_then(Density::from_id)
                .unwrap_or_default();
            updated.logging_format = log_format
                .active_id()
                .as_deref()
                .and_then(LogFormat::from_id)
                .unwrap_or_default();
            updated.logging_scope = log_scope
                .active_id()
                .as_deref()
                .and_then(LogScope::from_id)
                .unwrap_or_default();
            updated.close_to_tray = close_to_tray.is_active();
            updated.start_hidden = start_hidden.is_active();
            if plasma_placement_available {
                updated.plasma_window_placement = plasma_window_placement.is_active();
            }
            updated.favorites_only = favorites_only.is_active();
            updated.show_sensor_groups = show_sensor_groups.is_active();
            updated.show_identifying_information = show_identifying_information.is_active();
            updated.history_retention_seconds = history_retention.duration().as_secs();
            for (id, visible) in [
                ("current", current_column.is_active()),
                ("minimum", minimum_column.is_active()),
                ("maximum", maximum_column.is_active()),
                ("average", average_column.is_active()),
                ("status", status_column.is_active()),
            ] {
                table_for_response.set_column_visible(id, visible);
            }
            updated.columns = table_for_response.capture_columns();
            on_apply(updated);
        }
        dialog.close();
    });
    dialog.present();
}

pub(crate) fn show_hidden_items(
    parent: &ApplicationWindow,
    hidden: Vec<(String, String)>,
    on_show: impl Fn(String) + 'static,
    on_show_all: impl Fn() + 'static,
) {
    let dialog = Dialog::builder()
        .title("Hidden sensors and headers")
        .transient_for(parent)
        .modal(true)
        .use_header_bar(1)
        .default_width(520)
        .default_height(420)
        .build();
    dialog.add_button("Close", ResponseType::Close);

    let list = ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    let has_hidden = !hidden.is_empty();
    if has_hidden {
        let on_show = Rc::new(on_show);
        for (key, label) in hidden {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.add_css_class("hidden-item-row");
            let text = Label::new(Some(&label));
            text.set_xalign(0.0);
            text.set_hexpand(true);
            let show = gtk::Button::with_label("Show");
            row.append(&text);
            row.append(&show);
            list.append(&row);

            let callback = on_show.clone();
            let dialog_for_show = dialog.clone();
            show.connect_clicked(move |_| {
                (callback)(key.clone());
                dialog_for_show.close();
            });
        }
    } else {
        let label = Label::new(Some("No sensors or headers are hidden."));
        label.set_margin_top(24);
        label.set_margin_bottom(24);
        list.append(&label);
    }

    let show_all = gtk::Button::with_label("Show all hidden items");
    show_all.set_sensitive(has_hidden);
    let dialog_for_all = dialog.clone();
    show_all.connect_clicked(move |_| {
        on_show_all();
        dialog_for_all.close();
    });

    let scroller = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .child(&list)
        .build();
    scroller.set_overlay_scrolling(false);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    body.set_margin_top(10);
    body.set_margin_bottom(10);
    body.set_margin_start(10);
    body.set_margin_end(10);
    body.append(&scroller);
    body.append(&show_all);
    dialog.content_area().append(&body);
    dialog.connect_response(|dialog, _| dialog.close());
    dialog.present();
}

pub(crate) fn show_device_order(
    parent: &ApplicationWindow,
    devices: Vec<(String, String)>,
    on_apply: impl Fn(Vec<String>) + 'static,
) {
    let dialog = Dialog::builder()
        .title("Organize device headers")
        .transient_for(parent)
        .modal(true)
        .use_header_bar(1)
        .default_width(560)
        .default_height(520)
        .build();
    dialog.add_button("Cancel", ResponseType::Cancel);
    dialog.add_button("Apply", ResponseType::Apply);

    let items = Rc::new(RefCell::new(devices));
    let list = ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    populate_device_list(&list, &items.borrow(), None);

    let move_up = gtk::Button::builder()
        .icon_name("go-up-symbolic")
        .tooltip_text("Move selected device up")
        .build();
    let move_down = gtk::Button::builder()
        .icon_name("go-down-symbolic")
        .tooltip_text("Move selected device down")
        .build();
    let controls = gtk::Box::new(gtk::Orientation::Vertical, 6);
    controls.append(&move_up);
    controls.append(&move_down);

    let scroller = gtk::ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .child(&list)
        .build();
    scroller.set_overlay_scrolling(false);
    let body = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    body.set_margin_top(10);
    body.set_margin_bottom(10);
    body.set_margin_start(10);
    body.set_margin_end(10);
    body.append(&scroller);
    body.append(&controls);
    dialog.content_area().append(&body);

    let items_for_up = items.clone();
    let list_for_up = list.clone();
    move_up.connect_clicked(move |_| {
        move_selected_device(&list_for_up, &items_for_up, -1);
    });

    let items_for_down = items.clone();
    let list_for_down = list.clone();
    move_down.connect_clicked(move |_| {
        move_selected_device(&list_for_down, &items_for_down, 1);
    });

    dialog.connect_response(move |dialog, response| {
        if response == ResponseType::Apply {
            on_apply(items.borrow().iter().map(|(id, _)| id.clone()).collect());
        }
        dialog.close();
    });
    dialog.present();
}

fn move_selected_device(list: &ListBox, devices: &RefCell<Vec<(String, String)>>, direction: i32) {
    let Some(row) = list.selected_row() else {
        return;
    };
    let current = row.index();
    let target = current + direction;
    let len = devices.borrow().len() as i32;
    if current < 0 || !(0..len).contains(&target) {
        return;
    }

    devices.borrow_mut().swap(current as usize, target as usize);
    populate_device_list(list, &devices.borrow(), Some(target as usize));
}

fn populate_device_list(
    list: &ListBox,
    devices: &[(String, String)],
    selected_index: Option<usize>,
) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    for (_, name) in devices {
        let label = Label::new(Some(name));
        label.set_xalign(0.0);
        label.set_margin_top(5);
        label.set_margin_bottom(5);
        label.set_margin_start(8);
        label.set_margin_end(8);
        let row = ListBoxRow::new();
        row.set_child(Some(&label));
        list.append(&row);
    }
    if let Some(index) = selected_index {
        if let Some(row) = list.row_at_index(index as i32) {
            list.select_row(Some(&row));
        }
    }
}
