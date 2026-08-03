use gtk::glib;
use gtk::prelude::*;
use std::time::Duration;

pub(crate) fn set_label_text(label: &gtk::Label, text: &str, color: Option<&str>) {
    if let Some(color) = color {
        let escaped = glib::markup_escape_text(text);
        label.set_markup(&format!("<span foreground=\"{color}\">{escaped}</span>"));
    } else {
        label.set_text(text);
    }
}

pub(crate) fn restore_scroll_position(scroller: &gtk::ScrolledWindow, value: f64) {
    let adjustment = scroller.vadjustment();
    glib::idle_add_local_once(move || {
        let maximum = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
        adjustment.set_value(value.clamp(adjustment.lower(), maximum));
    });
}

pub(crate) fn copy_text(button: &gtk::Button, text: &str, restored_tooltip: &str) {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    display.clipboard().set_text(text);
    button.set_icon_name("emblem-ok-symbolic");
    button.set_tooltip_text(Some("Copied to clipboard"));
    let weak_button = button.downgrade();
    let restored_tooltip = restored_tooltip.to_owned();
    glib::timeout_add_local(Duration::from_millis(1_500), move || {
        if let Some(button) = weak_button.upgrade() {
            button.set_icon_name("edit-copy-symbolic");
            button.set_tooltip_text(Some(&restored_tooltip));
        }
        glib::ControlFlow::Break
    });
}

pub(crate) fn attach_labeled<W: IsA<gtk::Widget>>(
    grid: &gtk::Grid,
    row: i32,
    text: &str,
    widget: &W,
) {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_halign(gtk::Align::Start);
    label.set_valign(gtk::Align::Center);
    grid.attach(&label, 0, row, 1, 1);
    grid.attach(widget, 1, row, 1, 1);
}
