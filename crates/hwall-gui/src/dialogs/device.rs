use super::*;
use std::fmt::Write;

pub(crate) fn show_device_details(
    parent: &ApplicationWindow,
    device: Device,
    display_name: String,
    generated_name: String,
    storage_health_refreshable: bool,
    read_live: impl Fn() -> Option<Device> + 'static,
    on_refresh: impl Fn(String, bool) + 'static,
) -> Window {
    let window = details_window(parent, &display_name, 680, 680);
    let list = details_list();
    let refresh: Rc<dyn Fn(String, bool)> = Rc::new(on_refresh);
    render_device_details(
        &list,
        &device,
        &display_name,
        &generated_name,
        storage_health_refreshable,
        Rc::clone(&refresh),
    );

    let current = Rc::new(RefCell::new(device));
    let current_for_copy = Rc::clone(&current);
    let display_name_for_copy = display_name.clone();
    let generated_name_for_copy = generated_name.clone();
    let scroller = finish_details_window_with_copy(&window, &list, move || {
        device_details_text(
            &current_for_copy.borrow(),
            &display_name_for_copy,
            &generated_name_for_copy,
            storage_health_refreshable,
        )
    });
    let rendered_health_age = Rc::new(RefCell::new(storage_health_age_signature(
        &current.borrow(),
    )));
    let current_for_tick = Rc::clone(&current);
    let age_for_tick = Rc::clone(&rendered_health_age);
    let list_for_tick = list.clone();
    let scroller_for_tick = scroller.clone();
    let refresh_for_tick = Rc::clone(&refresh);
    let display_name_for_tick = display_name.clone();
    let generated_name_for_tick = generated_name.clone();
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
                &display_name_for_tick,
                &generated_name_for_tick,
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

fn device_details_text(
    device: &Device,
    display_name: &str,
    generated_name: &str,
    storage_health_refreshable: bool,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{display_name}");
    for (title, details) in device_detail_sections(
        device,
        display_name,
        generated_name,
        storage_health_refreshable,
    ) {
        let _ = writeln!(out, "\n{title}");
        for (label, value) in details {
            let _ = writeln!(out, "{label}: {value}");
        }
    }
    out.trim_end().to_owned()
}

fn device_detail_sections(
    device: &Device,
    display_name: &str,
    generated_name: &str,
    storage_health_refreshable: bool,
) -> Vec<(&'static str, Vec<(String, String)>)> {
    let mut identity = Vec::new();
    if display_name != generated_name {
        identity.push(("Alias".to_owned(), display_name.to_owned()));
    }
    identity.extend([
        ("Original name".to_owned(), generated_name.to_owned()),
        ("Device ID".to_owned(), device.id.clone()),
        ("Class".to_owned(), device.class.display_name().to_owned()),
    ]);
    for (label, value) in [
        ("Vendor", device.vendor.as_deref()),
        ("Model", device.model.as_deref()),
        ("Driver", device.driver.as_deref()),
        ("Bus address", device.bus_address.as_deref()),
        ("Parent", device.parent.as_deref()),
    ] {
        if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
            identity.push((label.to_owned(), value.to_owned()));
        }
    }
    identity.push(("Sensors".to_owned(), device.sensors.len().to_string()));
    let mut sections = vec![("Identity", identity)];

    let ordinary = device
        .properties
        .iter()
        .filter(|(key, _)| !is_storage_health_property(key))
        .filter_map(|(key, value)| {
            format_property_value(key, value).map(|value| (humanize_key(key), value))
        })
        .collect::<Vec<_>>();
    if !ordinary.is_empty() {
        sections.push(("Device", ordinary));
    }

    let has_storage_health = device.storage_health.is_some()
        || device
            .properties
            .keys()
            .any(|key| is_storage_health_property(key));
    if device.class == DeviceClass::Storage && (storage_health_refreshable || has_storage_health) {
        let mut health = Vec::new();
        if let Some(state) = device.storage_health.as_ref() {
            health.push(("Health status".to_owned(), state.status.to_string()));
            health.push((
                "Refresh state".to_owned(),
                storage_health_availability_text(state.availability).to_owned(),
            ));
            if let Some(checked) = state.last_success_unix_ms {
                health.push(("Last checked".to_owned(), format_sample_age(checked)));
            } else if let Some(attempted) = state.last_attempt_unix_ms {
                health.push(("Last attempt".to_owned(), format_sample_age(attempted)));
            }
            if !state.sources.is_empty() {
                health.push(("Sources".to_owned(), state.sources.join(", ")));
            }
            if let Some(message) = state.message.as_deref() {
                health.push(("Note".to_owned(), message.to_owned()));
            }
        } else {
            health.push(("Health status".to_owned(), "Not checked".to_owned()));
        }
        for key in STORAGE_HEALTH_PROPERTY_KEYS {
            let Some(value) = device.properties.get(*key) else {
                continue;
            };
            if let Some(value) = format_property_value(key, value) {
                health.push((storage_health_property_label(key).to_owned(), value));
            }
        }
        sections.push(("SMART / Health", health));
    }
    sections
}

fn render_device_details(
    list: &ListBox,
    device: &Device,
    display_name: &str,
    generated_name: &str,
    storage_health_refreshable: bool,
    refresh: Rc<dyn Fn(String, bool)>,
) {
    clear_list(list);
    for (title, details) in device_detail_sections(
        device,
        display_name,
        generated_name,
        storage_health_refreshable,
    ) {
        append_section(list, title);
        for (label, value) in details {
            append_detail(list, &label, &value);
        }
    }

    if device.class == DeviceClass::Storage && storage_health_refreshable {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copied_device_details_use_the_same_identity_as_the_window() {
        let mut device = Device::new("cpu:0", DeviceClass::Cpu, "Example CPU");
        device.vendor = Some("Example vendor".to_owned());

        let text = device_details_text(&device, "Main processor", "Example CPU", false);

        assert!(text.starts_with("Main processor\n\nIdentity\n"));
        assert!(text.contains("Alias: Main processor"));
        assert!(text.contains("Original name: Example CPU"));
        assert!(text.contains("Device ID: cpu:0"));
        assert!(text.contains("Vendor: Example vendor"));
    }
}
