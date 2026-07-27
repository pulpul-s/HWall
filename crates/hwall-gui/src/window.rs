use crate::hardware::HardwareView;
use crate::table::SensorTable;
use gtk::prelude::*;
use gtk::{Application, ApplicationWindow, Orientation};
use hwall_app::{AppSettings, MAIN_WINDOW_TITLE};

pub(super) struct ToolbarActions {
    pub(super) reset: gtk::Button,
    pub(super) rediscover: gtk::Button,
    pub(super) hidden: gtk::Button,
    pub(super) organize: gtk::Button,
    pub(super) settings: gtk::Button,
}

#[derive(Clone)]
pub(super) struct Ui {
    pub(super) application: Application,
    pub(super) window: ApplicationWindow,
    pub(super) table: SensorTable,
    pub(super) sensor_scroller: gtk::ScrolledWindow,
    pub(super) hardware: HardwareView,
    pub(super) content_stack: gtk::Stack,
    pub(super) search: gtk::SearchEntry,
    pub(super) pause_button: gtk::Button,
    pub(super) logging_button: gtk::Button,
    pub(super) favorites_button: gtk::ToggleButton,
    pub(super) sensor_controls: gtk::Box,
    pub(super) status_left: gtk::Label,
    pub(super) status_right: gtk::Label,
}

pub(super) fn build(application: &Application, settings: &AppSettings) -> (Ui, ToolbarActions) {
    install_css();

    let table = SensorTable::new(&settings.columns);
    table.set_density_class(settings.density);
    let hardware = HardwareView::new();
    let window = ApplicationWindow::builder()
        .application(application)
        .title(MAIN_WINDOW_TITLE)
        .default_width(settings.window_width.max(720))
        .default_height(settings.window_height.max(480))
        .build();
    if settings.maximized {
        window.maximize();
    }

    let content_stack = gtk::Stack::new();
    content_stack.set_hexpand(true);
    content_stack.set_vexpand(true);

    let sensor_scroller = gtk::ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .child(&table.view)
        .build();
    content_stack.add_titled(&sensor_scroller, Some("sensors"), "Sensors");
    content_stack.add_titled(&hardware.widget, Some("hardware"), "Hardware");
    content_stack.set_visible_child_name("sensors");

    let switcher = gtk::StackSwitcher::new();
    switcher.set_stack(Some(&content_stack));
    switcher.set_halign(gtk::Align::Start);

    let search = gtk::SearchEntry::builder()
        .placeholder_text("Filter sensors")
        .hexpand(true)
        .css_classes(["hwall-search"])
        .build();
    let pause_button = icon_button("media-playback-pause-symbolic", "Pause updates");
    let logging_button = icon_button("media-record-symbolic", "Start logging");
    let actions = ToolbarActions {
        reset: icon_button(
            "view-refresh-symbolic",
            "Reset minimum, maximum and average",
        ),
        rediscover: icon_button("emblem-synchronizing-symbolic", "Rediscover hardware"),
        hidden: icon_button("view-reveal-symbolic", "Manage hidden items"),
        organize: icon_button("view-sort-ascending-symbolic", "Organize device headers"),
        settings: icon_button("emblem-system-symbolic", "Settings"),
    };
    let favorites_button = gtk::ToggleButton::builder()
        .icon_name("starred-symbolic")
        .tooltip_text("Show favorite sensors only")
        .active(settings.favorites_only)
        .build();

    let sensor_controls = gtk::Box::new(Orientation::Horizontal, 5);
    for button in [
        &actions.reset,
        &actions.hidden,
        &logging_button,
        &actions.organize,
        &actions.settings,
    ] {
        sensor_controls.append(button);
    }
    sensor_controls.append(&favorites_button);

    let toolbar = gtk::Box::new(Orientation::Horizontal, 5);
    toolbar.add_css_class("hwall-toolbar");
    toolbar.append(&switcher);
    toolbar.append(&pause_button);
    toolbar.append(&actions.rediscover);
    toolbar.append(&sensor_controls);
    toolbar.append(&search);

    let status_left = gtk::Label::new(Some("Discovering"));
    status_left.set_xalign(0.0);
    status_left.set_hexpand(true);
    let status_right = gtk::Label::new(None);
    status_right.set_xalign(1.0);
    status_right.add_css_class("hwall-status-right");
    let statusbar = gtk::Box::new(Orientation::Horizontal, 8);
    statusbar.add_css_class("hwall-statusbar");
    statusbar.append(&status_left);
    statusbar.append(&status_right);

    let root = gtk::Box::new(Orientation::Vertical, 0);
    root.add_css_class("hwall-root");
    root.append(&toolbar);
    root.append(&content_stack);
    root.append(&statusbar);
    window.set_child(Some(&root));

    (
        Ui {
            application: application.clone(),
            window,
            table,
            sensor_scroller,
            hardware,
            content_stack,
            search,
            pause_button,
            logging_button,
            favorites_button,
            sensor_controls,
            status_left,
            status_right,
        },
        actions,
    )
}

fn install_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(include_str!("../resources/style.css"));
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .build()
}
