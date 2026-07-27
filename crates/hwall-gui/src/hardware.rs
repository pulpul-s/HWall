use crate::ui::{restore_scroll_position, set_label_text};
use gtk::prelude::*;
use gtk::{Align, Orientation, SelectionMode};
use hwall_app::{HardwareDevice, HardwareInventory, HardwareProperty, HardwareSensor};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

#[derive(Clone)]
pub(super) struct HardwareView {
    pub(super) widget: gtk::Box,
    navigation: gtk::ListBox,
    details: gtk::Box,
    details_scroller: gtk::ScrolledWindow,
    state: Rc<RefCell<HardwareState>>,
}

#[derive(Default)]
struct HardwareState {
    inventory: Option<HardwareInventory>,
    query: String,
    rows: Vec<(gtk::ListBoxRow, String)>,
    navigation_signature: Vec<(String, String, String)>,
    selected_id: Option<String>,
    rendered: Option<HardwareDevice>,
    property_labels: BTreeMap<String, gtk::Label>,
    sensor_labels: BTreeMap<String, SensorLabels>,
    refresh_handler: Option<Rc<dyn Fn(String, bool)>>,
    rebuilding_navigation: bool,
}

#[derive(Clone)]
struct SensorLabels {
    current: gtk::Label,
    minimum: gtk::Label,
    maximum: gtk::Label,
    average: gtk::Label,
    status: gtk::Label,
}

impl HardwareView {
    pub(super) fn new() -> Self {
        let navigation = gtk::ListBox::new();
        navigation.set_selection_mode(SelectionMode::Single);
        navigation.set_activate_on_single_click(false);
        navigation.add_css_class("hardware-navigation");

        let navigation_scroller = gtk::ScrolledWindow::builder()
            .hexpand(false)
            .vexpand(true)
            .min_content_width(230)
            .child(&navigation)
            .build();

        let details = gtk::Box::new(Orientation::Vertical, 14);
        details.set_margin_top(18);
        details.set_margin_bottom(18);
        details.set_margin_start(18);
        details.set_margin_end(18);
        details.set_hexpand(true);
        details.set_vexpand(true);
        show_empty_state(&details, "Select a hardware device");

        let details_scroller = gtk::ScrolledWindow::builder()
            .hexpand(true)
            .vexpand(true)
            .child(&details)
            .build();

        let paned = gtk::Paned::new(Orientation::Horizontal);
        paned.set_start_child(Some(&navigation_scroller));
        paned.set_end_child(Some(&details_scroller));
        paned.set_position(260);
        paned.set_resize_start_child(false);
        paned.set_shrink_start_child(false);
        paned.set_hexpand(true);
        paned.set_vexpand(true);

        let widget = gtk::Box::new(Orientation::Vertical, 0);
        widget.append(&paned);

        let state = Rc::new(RefCell::new(HardwareState::default()));

        let state_for_selection = Rc::clone(&state);
        let details_for_selection = details.clone();
        navigation.connect_row_selected(move |_, selected| {
            let Some(selected) = selected else {
                return;
            };
            let device = select_device_for_row(&state_for_selection, selected);
            if let Some(device) = device {
                render_device(&state_for_selection, &details_for_selection, &device);
            }
        });

        Self {
            widget,
            navigation,
            details,
            details_scroller,
            state,
        }
    }

    pub(super) fn sync(&self, inventory: HardwareInventory) {
        let (navigation_changed, query) = {
            let mut state = self.state.borrow_mut();
            let signature = navigation_signature(&inventory, &state.query);
            let changed = state.navigation_signature != signature;
            state.inventory = Some(inventory);
            (changed, state.query.clone())
        };

        if navigation_changed {
            rebuild_navigation(&self.state, &self.navigation, &self.details, &query);
            return;
        }

        let selected = {
            let state = self.state.borrow();
            state.selected_id.as_deref().and_then(|id| {
                state
                    .inventory
                    .as_ref()
                    .and_then(|inventory| inventory.device(id))
                    .cloned()
            })
        };
        if let Some(device) = selected {
            update_or_render_device(
                &self.state,
                &self.details,
                Some(&self.details_scroller),
                &device,
            );
        }
    }

    pub(super) fn set_query(&self, query: &str) {
        let changed = {
            let mut state = self.state.borrow_mut();
            if state.query == query {
                false
            } else {
                state.query = query.to_owned();
                true
            }
        };
        if changed {
            rebuild_navigation(&self.state, &self.navigation, &self.details, query);
        }
    }

    pub(super) fn set_refresh_health_handler(&self, handler: impl Fn(String, bool) + 'static) {
        self.state.borrow_mut().refresh_handler = Some(Rc::new(handler));
    }

    pub(super) fn device_count(&self) -> usize {
        self.state
            .borrow()
            .inventory
            .as_ref()
            .map(HardwareInventory::device_count)
            .unwrap_or(0)
    }
}

fn rebuild_navigation(
    state: &Rc<RefCell<HardwareState>>,
    navigation: &gtk::ListBox,
    details: &gtk::Box,
    query: &str,
) {
    let (inventory, selected_id) = {
        let mut state = state.borrow_mut();
        let Some(inventory) = state.inventory.clone() else {
            return;
        };
        state.rebuilding_navigation = true;
        (inventory, state.selected_id.clone())
    };

    while let Some(child) = navigation.first_child() {
        navigation.remove(&child);
    }

    let mut rows = Vec::new();
    let mut first_row = None;
    let mut selected_row = None;
    let mut first_category = true;
    for category in &inventory.categories {
        let devices = category
            .devices
            .iter()
            .filter(|device| device.matches(query))
            .collect::<Vec<_>>();
        if devices.is_empty() {
            continue;
        }

        let category_row = gtk::ListBoxRow::new();
        category_row.set_selectable(false);
        category_row.set_activatable(false);

        let category_box = gtk::Box::new(Orientation::Vertical, 0);
        if !first_category {
            let separator = gtk::Separator::new(Orientation::Horizontal);
            separator.add_css_class("hardware-category-separator");
            category_box.append(&separator);
        }
        let category_label = gtk::Label::new(Some(&category.label));
        category_label.set_xalign(0.0);
        category_label.add_css_class("hardware-category");
        category_box.append(&category_label);
        category_row.set_child(Some(&category_box));
        navigation.append(&category_row);
        first_category = false;

        for device in devices {
            let row = hardware_device_row(device);
            if first_row.is_none() {
                first_row = Some(row.clone());
            }
            if selected_id.as_deref() == Some(device.id.as_str()) {
                selected_row = Some(row.clone());
            }
            rows.push((row.clone(), device.id.clone()));
            navigation.append(&row);
        }
    }

    let signature = navigation_signature(&inventory, query);
    {
        let mut state = state.borrow_mut();
        state.rows = rows;
        state.navigation_signature = signature;
        state.rebuilding_navigation = false;
    }

    let target = selected_row.or(first_row);
    if let Some(row) = target {
        navigation.select_row(Some(&row));
        let device = select_device_for_row(state, &row);
        if let Some(device) = device {
            update_or_render_device(state, details, None, &device);
        }
    } else {
        let mut state = state.borrow_mut();
        state.selected_id = None;
        state.rendered = None;
        state.property_labels.clear();
        state.sensor_labels.clear();
        drop(state);
        show_empty_state(details, "No hardware matches the search");
    }
}

fn select_device_for_row(
    state: &Rc<RefCell<HardwareState>>,
    row: &gtk::ListBoxRow,
) -> Option<HardwareDevice> {
    let mut state = state.borrow_mut();
    if state.rebuilding_navigation {
        return None;
    }
    let id = state
        .rows
        .iter()
        .find(|(known, _)| known == row)
        .map(|(_, id)| id.clone())?;
    state.selected_id = Some(id.clone());
    state
        .inventory
        .as_ref()
        .and_then(|inventory| inventory.device(&id))
        .cloned()
}

fn navigation_signature(
    inventory: &HardwareInventory,
    query: &str,
) -> Vec<(String, String, String)> {
    let mut signature = Vec::new();
    for category in &inventory.categories {
        for device in category
            .devices
            .iter()
            .filter(|device| device.matches(query))
        {
            signature.push((
                category.label.clone(),
                device.id.clone(),
                format!("{}\n{}", device.name, device.subtitle),
            ));
        }
    }
    signature
}

fn hardware_device_row(device: &HardwareDevice) -> gtk::ListBoxRow {
    let title = gtk::Label::new(Some(&device.name));
    title.set_xalign(0.0);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    title.add_css_class("hardware-device-title");

    let subtitle = gtk::Label::new(Some(&device.subtitle));
    subtitle.set_xalign(0.0);
    subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
    subtitle.add_css_class("hardware-device-subtitle");

    let content = gtk::Box::new(Orientation::Vertical, 2);
    content.append(&title);
    content.append(&subtitle);

    let row = gtk::ListBoxRow::new();
    row.set_activatable(false);
    row.set_selectable(true);
    row.set_child(Some(&content));
    row
}

fn update_or_render_device(
    state: &Rc<RefCell<HardwareState>>,
    details: &gtk::Box,
    scroller: Option<&gtk::ScrolledWindow>,
    device: &HardwareDevice,
) {
    let requires_render = {
        let state = state.borrow();
        state
            .rendered
            .as_ref()
            .is_none_or(|rendered| !same_static_device(rendered, device))
    };
    if requires_render {
        let scroll_position = scroller.map(|scroller| scroller.vadjustment().value());
        render_device(state, details, device);
        if let (Some(scroller), Some(scroll_position)) = (scroller, scroll_position) {
            restore_scroll_position(scroller, scroll_position);
        }
    } else {
        update_detail_labels(state, device);
        state.borrow_mut().rendered = Some(device.clone());
    }
}

fn same_static_device(left: &HardwareDevice, right: &HardwareDevice) -> bool {
    left.id == right.id
        && left.name == right.name
        && left.subtitle == right.subtitle
        && left.storage_health_refreshable == right.storage_health_refreshable
        && left.storage_health_permission_required == right.storage_health_permission_required
        && same_sections(&left.sections, &right.sections)
        && same_properties(&left.advanced, &right.advanced)
        && left
            .sensors
            .iter()
            .map(sensor_identity)
            .eq(right.sensors.iter().map(sensor_identity))
}

fn same_sections(
    left: &[hwall_app::HardwareSection],
    right: &[hwall_app::HardwareSection],
) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.title == right.title && same_properties(&left.properties, &right.properties)
        })
}

fn same_properties(left: &[HardwareProperty], right: &[HardwareProperty]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| left.key == right.key && left.label == right.label)
}

fn sensor_identity(sensor: &HardwareSensor) -> (&str, &str, &str) {
    (&sensor.id, &sensor.group, &sensor.label)
}

fn render_device(state: &Rc<RefCell<HardwareState>>, details: &gtk::Box, device: &HardwareDevice) {
    clear_box(details);

    let title = gtk::Label::new(Some(&device.name));
    title.set_xalign(0.0);
    title.set_wrap(true);
    title.add_css_class("hardware-details-title");
    details.append(&title);

    if !device.subtitle.is_empty() {
        let subtitle = gtk::Label::new(Some(&device.subtitle));
        subtitle.set_xalign(0.0);
        subtitle.set_wrap(true);
        subtitle.add_css_class("dim-label");
        details.append(&subtitle);
    }

    let mut property_labels = BTreeMap::new();
    for section in &device.sections {
        append_property_section(
            details,
            &section.title,
            &section.properties,
            &mut property_labels,
        );
        if section.title == "SMART / Health" && device.storage_health_refreshable {
            append_health_refresh_button(state, details, device);
        }
    }

    let sensor_labels = append_sensor_section(details, &device.sensors);

    if !device.advanced.is_empty() {
        let heading = gtk::Label::new(Some("Advanced"));
        heading.set_xalign(0.0);
        heading.add_css_class("heading");
        details.append(&heading);
        details.append(&property_grid(
            "Advanced",
            &device.advanced,
            &mut property_labels,
            true,
        ));
    }

    let mut state = state.borrow_mut();
    state.rendered = Some(device.clone());
    state.property_labels = property_labels;
    state.sensor_labels = sensor_labels;
}

fn append_health_refresh_button(
    state: &Rc<RefCell<HardwareState>>,
    details: &gtk::Box,
    device: &HardwareDevice,
) {
    let Some(handler) = state.borrow().refresh_handler.clone() else {
        return;
    };
    let elevated = device.storage_health_permission_required;
    let button = gtk::Button::with_label(if elevated {
        "Refresh as administrator"
    } else {
        "Refresh health"
    });
    button.set_halign(Align::Start);
    button.set_focus_on_click(false);
    button.set_tooltip_text(Some(if elevated {
        "Authorize one read-only SMART / NVMe health refresh"
    } else {
        "Refresh SMART / NVMe health information"
    }));
    let device_id = device.id.clone();
    button.connect_clicked(move |_| handler(device_id.clone(), elevated));
    details.append(&button);
}

fn append_property_section(
    details: &gtk::Box,
    title: &str,
    properties: &[HardwareProperty],
    labels: &mut BTreeMap<String, gtk::Label>,
) {
    if properties.is_empty() {
        return;
    }
    let heading = gtk::Label::new(Some(title));
    heading.set_xalign(0.0);
    heading.add_css_class("heading");
    details.append(&heading);
    details.append(&property_grid(title, properties, labels, false));
}

fn property_grid(
    section: &str,
    properties: &[HardwareProperty],
    labels: &mut BTreeMap<String, gtk::Label>,
    raw_keys: bool,
) -> gtk::Grid {
    let grid = gtk::Grid::new();
    grid.set_column_spacing(18);
    grid.set_row_spacing(5);
    grid.set_hexpand(true);

    for (row, property) in properties.iter().enumerate() {
        let key_text = if raw_keys {
            &property.key
        } else {
            &property.label
        };
        let key = gtk::Label::new(Some(key_text));
        key.set_xalign(0.0);
        key.set_yalign(0.0);
        key.add_css_class("dim-label");
        if raw_keys {
            key.set_tooltip_text(Some(&property.label));
        }

        let value = gtk::Label::new(Some(&property.value));
        value.set_xalign(0.0);
        value.set_yalign(0.0);
        value.set_hexpand(true);
        value.set_wrap(true);
        value.set_selectable(true);

        grid.attach(&key, 0, row as i32, 1, 1);
        grid.attach(&value, 1, row as i32, 1, 1);
        labels.insert(property_widget_key(section, &property.key), value);
    }
    grid
}

fn property_widget_key(section: &str, key: &str) -> String {
    format!("{section}\u{1f}{key}")
}

fn append_sensor_section(
    details: &gtk::Box,
    sensors: &[HardwareSensor],
) -> BTreeMap<String, SensorLabels> {
    let mut labels = BTreeMap::new();
    if sensors.is_empty() {
        return labels;
    }

    let heading = gtk::Label::new(Some(&format!("Related sensors — {}", sensors.len())));
    heading.set_xalign(0.0);
    heading.add_css_class("heading");
    details.append(&heading);

    let grid = gtk::Grid::new();
    grid.set_column_spacing(14);
    grid.set_row_spacing(4);
    grid.set_hexpand(true);
    for (column, title) in [
        "Reading", "Current", "Minimum", "Maximum", "Average", "Status",
    ]
    .into_iter()
    .enumerate()
    {
        let label = gtk::Label::new(Some(title));
        label.set_xalign(if column == 0 { 0.0 } else { 1.0 });
        label.add_css_class("dim-label");
        grid.attach(&label, column as i32, 0, 1, 1);
    }

    let mut row = 1;
    let mut current_group = None;
    for sensor in sensors {
        if current_group != Some(sensor.group.as_str()) {
            current_group = Some(sensor.group.as_str());
            let group = gtk::Label::new(Some(&sensor.group));
            group.set_xalign(0.0);
            group.add_css_class("hardware-sensor-group");
            grid.attach(&group, 0, row, 6, 1);
            row += 1;
        }

        let name = gtk::Label::new(Some(&sensor.label));
        name.set_xalign(0.0);
        name.set_hexpand(true);
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);

        let current = sensor_value_label(&sensor.current, sensor.current_color.as_deref());
        let minimum = sensor_value_label(&sensor.minimum, sensor.minimum_color.as_deref());
        let maximum = sensor_value_label(&sensor.maximum, sensor.maximum_color.as_deref());
        let average = sensor_value_label(&sensor.average, sensor.average_color.as_deref());
        let status = sensor_value_label(&sensor.status, sensor.status_color.as_deref());
        status.set_tooltip_text(Some(&sensor.status));

        for (column, label) in [
            (0, &name),
            (1, &current),
            (2, &minimum),
            (3, &maximum),
            (4, &average),
            (5, &status),
        ] {
            grid.attach(label, column, row, 1, 1);
        }

        labels.insert(
            sensor.id.clone(),
            SensorLabels {
                current,
                minimum,
                maximum,
                average,
                status,
            },
        );
        row += 1;
    }
    details.append(&grid);
    labels
}

fn sensor_value_label(value: &str, color: Option<&str>) -> gtk::Label {
    let label = gtk::Label::new(None);
    set_label_text(&label, value, color);
    label.set_xalign(1.0);
    label.set_selectable(true);
    label.add_css_class("numeric-cell");
    label
}

fn update_detail_labels(state: &Rc<RefCell<HardwareState>>, device: &HardwareDevice) {
    let state = state.borrow();
    for section in &device.sections {
        for property in &section.properties {
            let key = property_widget_key(&section.title, &property.key);
            if let Some(label) = state.property_labels.get(&key) {
                label.set_text(&property.value);
            }
        }
    }
    for property in &device.advanced {
        let key = property_widget_key("Advanced", &property.key);
        if let Some(label) = state.property_labels.get(&key) {
            label.set_text(&property.value);
        }
    }
    for sensor in &device.sensors {
        let Some(labels) = state.sensor_labels.get(&sensor.id) else {
            continue;
        };
        set_label_text(
            &labels.current,
            &sensor.current,
            sensor.current_color.as_deref(),
        );
        set_label_text(
            &labels.minimum,
            &sensor.minimum,
            sensor.minimum_color.as_deref(),
        );
        set_label_text(
            &labels.maximum,
            &sensor.maximum,
            sensor.maximum_color.as_deref(),
        );
        set_label_text(
            &labels.average,
            &sensor.average,
            sensor.average_color.as_deref(),
        );
        set_label_text(
            &labels.status,
            &sensor.status,
            sensor.status_color.as_deref(),
        );
        labels.status.set_tooltip_text(Some(&sensor.status));
    }
}

fn show_empty_state(details: &gtk::Box, message: &str) {
    clear_box(details);
    let label = gtk::Label::new(Some(message));
    label.set_halign(Align::Center);
    label.set_valign(Align::Center);
    label.set_hexpand(true);
    label.set_vexpand(true);
    label.add_css_class("dim-label");
    details.append(&label);
}

fn clear_box(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}
