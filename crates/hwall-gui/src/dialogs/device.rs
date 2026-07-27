use super::*;

pub(crate) fn show_device_details(
    parent: &ApplicationWindow,
    device: Device,
    storage_health_refreshable: bool,
    read_live: impl Fn() -> Option<Device> + 'static,
    on_refresh: impl Fn(String, bool) + 'static,
) -> Window {
    let window = details_window(parent, &device.name, 680, 680);
    let list = details_list();
    let refresh: Rc<dyn Fn(String, bool)> = Rc::new(on_refresh);
    render_device_details(
        &list,
        &device,
        storage_health_refreshable,
        Rc::clone(&refresh),
    );

    let scroller = finish_details_window(&window, &list);
    let rendered_health_age = Rc::new(RefCell::new(storage_health_age_signature(&device)));
    let current = Rc::new(RefCell::new(device));
    let current_for_tick = Rc::clone(&current);
    let age_for_tick = Rc::clone(&rendered_health_age);
    let list_for_tick = list.clone();
    let scroller_for_tick = scroller.clone();
    let refresh_for_tick = Rc::clone(&refresh);
    let weak_window = window.downgrade();
    glib::timeout_add_local(Duration::from_millis(500), move || {
        if weak_window.upgrade().is_none() {
            return glib::ControlFlow::Break;
        }
        let Some(next) = read_live() else {
            return glib::ControlFlow::Continue;
        };
        let changed = {
            let current = current_for_tick.borrow();
            device_details_changed(&current, &next)
        };
        let next_age = storage_health_age_signature(&next);
        let age_changed = *age_for_tick.borrow() != next_age;
        if changed || age_changed {
            let scroll_position = scroller_for_tick.vadjustment().value();
            render_device_details(
                &list_for_tick,
                &next,
                storage_health_refreshable,
                Rc::clone(&refresh_for_tick),
            );
            restore_scroll_position(&scroller_for_tick, scroll_position);
            *current_for_tick.borrow_mut() = next;
            *age_for_tick.borrow_mut() = next_age;
        }
        glib::ControlFlow::Continue
    });

    window
}

fn render_device_details(
    list: &ListBox,
    device: &Device,
    storage_health_refreshable: bool,
    refresh: Rc<dyn Fn(String, bool)>,
) {
    clear_list(list);
    append_section(list, "Identity");
    append_detail(list, "Device ID", &device.id);
    append_detail(list, "Class", device.class.display_name());
    append_optional_detail(list, "Vendor", device.vendor.as_deref());
    append_optional_detail(list, "Model", device.model.as_deref());
    append_optional_detail(list, "Driver", device.driver.as_deref());
    append_optional_detail(list, "Bus address", device.bus_address.as_deref());
    append_optional_detail(list, "Parent", device.parent.as_deref());
    append_detail(list, "Sensors", &device.sensors.len().to_string());

    let ordinary = device
        .properties
        .iter()
        .filter(|(key, _)| !is_storage_health_property(key))
        .collect::<Vec<_>>();
    if !ordinary.is_empty() {
        append_section(list, "Device");
        for (key, value) in ordinary {
            if let Some(rendered) = format_property_value(key, value) {
                append_detail(list, &humanize_key(key), &rendered);
            }
        }
    }

    let has_storage_health = device.storage_health.is_some()
        || device
            .properties
            .keys()
            .any(|key| is_storage_health_property(key));
    if device.class == DeviceClass::Storage && (storage_health_refreshable || has_storage_health) {
        append_section(list, "SMART / Health");
        append_storage_health(list, device.storage_health.as_ref());
        for key in STORAGE_HEALTH_PROPERTY_KEYS {
            let Some(value) = device.properties.get(*key) else {
                continue;
            };
            if let Some(rendered) = format_property_value(key, value) {
                append_detail(list, storage_health_property_label(key), &rendered);
            }
        }
        if storage_health_refreshable {
            let elevated = device.storage_health.as_ref().is_some_and(|health| {
                health.availability == StorageHealthAvailability::PermissionDenied
            });
            let refresh_button = gtk::Button::with_label(if elevated {
                "Refresh as administrator"
            } else {
                "Refresh health"
            });
            refresh_button.set_halign(Align::Start);
            refresh_button.set_focus_on_click(false);
            refresh_button.set_margin_top(6);
            refresh_button.set_margin_bottom(6);
            refresh_button.set_margin_start(12);
            refresh_button.set_tooltip_text(Some(if elevated {
                "Authorize one read-only SMART / NVMe health refresh"
            } else {
                "Refresh SMART / NVMe health information"
            }));
            let device_id = device.id.clone();
            refresh_button.connect_clicked(move |_| refresh(device_id.clone(), elevated));
            list.append(&refresh_button);
        }
    }
}

fn append_storage_health(list: &ListBox, health: Option<&StorageHealth>) {
    let Some(health) = health else {
        append_detail(list, "Health status", "Not checked");
        return;
    };
    append_detail(list, "Health status", &health.status.to_string());
    append_detail(
        list,
        "Refresh state",
        storage_health_availability_text(health.availability),
    );
    if let Some(checked) = health.last_success_unix_ms {
        append_detail(list, "Last checked", &format_sample_age(checked));
    } else if let Some(attempted) = health.last_attempt_unix_ms {
        append_detail(list, "Last attempt", &format_sample_age(attempted));
    }
    if !health.sources.is_empty() {
        append_detail(list, "Sources", &health.sources.join(", "));
    }
    if let Some(message) = health.message.as_deref() {
        append_detail(list, "Note", message);
    }
}

fn storage_health_age_signature(device: &Device) -> Option<String> {
    let health = device.storage_health.as_ref()?;
    health
        .last_success_unix_ms
        .or(health.last_attempt_unix_ms)
        .map(format_sample_age)
}

fn device_details_changed(current: &Device, next: &Device) -> bool {
    current.name != next.name
        || current.vendor != next.vendor
        || current.model != next.model
        || current.driver != next.driver
        || current.bus_address != next.bus_address
        || current.parent != next.parent
        || current.properties != next.properties
        || current.storage_health != next.storage_health
        || current.sensors.len() != next.sensors.len()
}

fn clear_list(list: &ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}
