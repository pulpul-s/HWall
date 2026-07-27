use crate::history::{SensorKey, SharedHistory};
use crate::table::SensorTable;
use crate::ui::{attach_labeled, restore_scroll_position, set_label_text};
use gtk::prelude::*;
use gtk::{
    glib, Align, ApplicationWindow, CheckButton, ComboBoxText, Dialog, Grid, Label, ListBox,
    ListBoxRow, ResponseType, Window,
};
use hwall_app::{
    alert_supported_sensor, present_sensor, rule_summary, storage_health_availability_text,
    AlertRule, AlertState, AppSettings, Density, LogFormat, LogScope, SensorRow,
};
use hwall_core::render::{
    format_property_value, format_sample_age, format_value, humanize_key, sensor_kind_name,
    storage_health_property_label,
};
use hwall_core::{
    is_storage_health_property, Device, DeviceClass, Identification, RunningStatistics, Sensor,
    StorageHealth, StorageHealthAvailability, STORAGE_HEALTH_PROPERTY_KEYS,
};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

mod device;
mod preferences;
mod sensor;

pub(super) use device::show_device_details;
pub(super) use preferences::{
    show_device_order, show_hidden_items, show_sensor_alias, show_settings,
};
pub(super) use sensor::{show_sensor_details, SensorDetailsRequest};

fn details_window(parent: &ApplicationWindow, title: &str, width: i32, height: i32) -> Window {
    let window = Window::builder()
        .title(title)
        .transient_for(parent)
        .modal(false)
        .destroy_with_parent(true)
        .default_width(width)
        .default_height(height)
        .build();
    if let Some(application) = parent.application() {
        window.set_application(Some(&application));
    }
    window
}

fn finish_details_window(window: &Window, list: &ListBox) -> gtk::ScrolledWindow {
    let scroller = gtk::ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .child(list)
        .build();
    scroller.set_margin_top(10);
    scroller.set_margin_bottom(10);
    scroller.set_margin_start(10);
    scroller.set_margin_end(10);
    window.set_child(Some(&scroller));
    window.present();
    scroller
}

fn details_list() -> ListBox {
    let list = ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("boxed-list");
    list
}

fn append_section(list: &ListBox, text: &str) {
    let label = Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_margin_top(12);
    label.set_margin_bottom(4);
    label.set_margin_start(12);
    label.set_margin_end(12);
    label.add_css_class("heading");
    list.append(&label);
}

fn append_optional_detail(list: &ListBox, name: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        append_detail(list, name, value);
    }
}

fn append_detail(list: &ListBox, name: &str, value: &str) {
    append_detail_value(list, name, value);
}

fn append_detail_value(list: &ListBox, name: &str, value: &str) -> Label {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    row.set_margin_top(6);
    row.set_margin_bottom(6);
    row.set_margin_start(12);
    row.set_margin_end(12);

    let key = Label::new(Some(name));
    key.set_xalign(0.0);
    key.set_valign(Align::Start);
    key.set_width_chars(22);
    key.add_css_class("dim-label");

    let value = Label::new(Some(value));
    value.set_xalign(0.0);
    value.set_hexpand(true);
    value.set_wrap(true);
    value.set_selectable(true);

    row.append(&key);
    row.append(&value);
    list.append(&row);
    value
}

fn identification_name(identification: Identification) -> &'static str {
    match identification {
        Identification::KernelLabel => "Kernel label",
        Identification::FirmwareLabel => "Firmware label",
        Identification::LibSensorsConfig => "lm-sensors configuration",
        Identification::VendorApi => "Vendor API",
        Identification::KnownDriverMapping => "Known driver mapping",
        Identification::BoardDatabase => "Board database",
        Identification::Inferred => "Derived or inferred",
        Identification::Unidentified => "Unidentified",
    }
}
