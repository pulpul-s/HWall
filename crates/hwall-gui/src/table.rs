use crate::ui::set_label_text;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::{ColumnView, ColumnViewColumn, Label, MultiSelection, SignalListItemFactory};
use hwall_app::{ColumnSettings, Density, RowKind, SensorRow};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

const SENSOR_MARKER_SLOT: &str = "    ";

type ContextHandler = Rc<dyn Fn(SensorRow, gtk::Widget, f64, f64)>;

#[derive(Default)]
struct TableHandlers {
    context: Option<ContextHandler>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DataColumn {
    Sensor,
    Current,
    Minimum,
    Maximum,
    Average,
    Status,
}

impl DataColumn {
    fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|column| column.id() == id)
    }

    fn metadata(self) -> (&'static str, &'static str) {
        match self {
            Self::Sensor => ("sensor", "Sensor"),
            Self::Current => ("current", "Current"),
            Self::Minimum => ("minimum", "Minimum"),
            Self::Maximum => ("maximum", "Maximum"),
            Self::Average => ("average", "Average"),
            Self::Status => ("status", "Status"),
        }
    }

    fn id(self) -> &'static str {
        self.metadata().0
    }

    fn title(self) -> &'static str {
        self.metadata().1
    }

    fn text<'a>(self, row: &'a SensorRow) -> Cow<'a, str> {
        match self {
            Self::Sensor => Cow::Borrowed(&row.label),
            Self::Current => Cow::Borrowed(&row.current),
            Self::Minimum => Cow::Borrowed(&row.minimum),
            Self::Maximum => Cow::Borrowed(&row.maximum),
            Self::Average => Cow::Borrowed(&row.average),
            Self::Status => Cow::Borrowed(&row.status),
        }
    }

    fn color(self, row: &SensorRow) -> Option<&str> {
        match self {
            Self::Sensor => None,
            Self::Current => row.current_color.as_deref(),
            Self::Minimum => row.minimum_color.as_deref(),
            Self::Maximum => row.maximum_color.as_deref(),
            Self::Average => row.average_color.as_deref(),
            Self::Status => row.status_color.as_deref(),
        }
    }

    fn numeric(self) -> bool {
        matches!(
            self,
            Self::Current | Self::Minimum | Self::Maximum | Self::Average
        )
    }

    fn index(self) -> usize {
        match self {
            Self::Sensor => 0,
            Self::Current => 1,
            Self::Minimum => 2,
            Self::Maximum => 3,
            Self::Average => 4,
            Self::Status => 5,
        }
    }

    const ALL: [Self; 6] = [
        Self::Sensor,
        Self::Current,
        Self::Minimum,
        Self::Maximum,
        Self::Average,
        Self::Status,
    ];
}

struct RowState {
    row: SensorRow,
    labels: [Option<glib::WeakRef<Label>>; 6],
    sensor_prefix: Option<glib::WeakRef<Label>>,
    favorite_marker: Option<glib::WeakRef<Label>>,
    favorite_slot: Option<glib::WeakRef<gtk::Overlay>>,
}

impl RowState {
    fn new(row: SensorRow) -> Self {
        Self {
            row,
            labels: std::array::from_fn(|_| None),
            sensor_prefix: None,
            favorite_marker: None,
            favorite_slot: None,
        }
    }

    fn bind_label(&mut self, kind: DataColumn, label: &Label) {
        self.labels[kind.index()] = Some(label.downgrade());
        render_label(kind, &self.row, label);
    }

    fn bind_sensor_prefix(&mut self, prefix: &Label, marker: &Label, slot: &gtk::Overlay) {
        self.sensor_prefix = Some(prefix.downgrade());
        self.favorite_marker = Some(marker.downgrade());
        self.favorite_slot = Some(slot.downgrade());
        render_sensor_prefix(&self.row, prefix, marker, slot);
    }

    fn unbind_label(&mut self, kind: DataColumn) {
        self.labels[kind.index()] = None;
        if kind == DataColumn::Sensor {
            self.sensor_prefix = None;
            self.favorite_marker = None;
            self.favorite_slot = None;
        }
    }

    fn update(&mut self, row: &SensorRow) {
        if self.row == *row {
            return;
        }
        self.row = row.clone();
        for kind in DataColumn::ALL {
            let index = kind.index();
            if let Some(label) = self.labels[index].as_ref().and_then(|weak| weak.upgrade()) {
                render_label(kind, &self.row, &label);
            } else {
                self.labels[index] = None;
            }
        }
        match (
            self.sensor_prefix.as_ref().and_then(|weak| weak.upgrade()),
            self.favorite_marker
                .as_ref()
                .and_then(|weak| weak.upgrade()),
            self.favorite_slot.as_ref().and_then(|weak| weak.upgrade()),
        ) {
            (Some(prefix), Some(marker), Some(slot)) => {
                render_sensor_prefix(&self.row, &prefix, &marker, &slot)
            }
            _ => {
                self.sensor_prefix = None;
                self.favorite_marker = None;
                self.favorite_slot = None;
            }
        }
    }
}

#[derive(Clone)]
pub(super) struct SensorTable {
    pub(super) view: ColumnView,
    store: gio::ListStore,
    selection: MultiSelection,
    columns: BTreeMap<String, ColumnViewColumn>,
    handlers: Rc<RefCell<TableHandlers>>,
}

impl SensorTable {
    pub(super) fn new(settings: &[ColumnSettings]) -> Self {
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = MultiSelection::new(Some(store.clone()));
        let view = ColumnView::new(Some(selection.clone()));
        view.set_reorderable(true);
        view.set_show_row_separators(true);
        view.set_show_column_separators(true);
        view.add_css_class("hwall-table");
        view.add_css_class("data-table");
        view.set_vexpand(true);
        view.set_hexpand(true);

        let handlers = Rc::new(RefCell::new(TableHandlers::default()));
        let sanitizing_selection = Rc::new(Cell::new(false));
        let guard = Rc::clone(&sanitizing_selection);
        selection.connect_selection_changed(move |selection, _, _| {
            if guard.replace(true) {
                return;
            }
            let selected_kinds = (0..selection.n_items())
                .filter(|index| selection.is_selected(*index))
                .filter_map(|index| selection.item(index))
                .filter_map(|item| item.downcast::<glib::BoxedAnyObject>().ok())
                .map(|item| item.borrow::<RowState>().row.kind)
                .collect::<Vec<_>>();
            if selected_kinds.len() > 1 && selected_kinds.contains(&RowKind::Sensor) {
                for (index, item) in (0..selection.n_items())
                    .filter(|index| selection.is_selected(*index))
                    .filter_map(|index| selection.item(index).map(|item| (index, item)))
                {
                    let remove = item
                        .downcast::<glib::BoxedAnyObject>()
                        .ok()
                        .is_some_and(|item| item.borrow::<RowState>().row.kind != RowKind::Sensor);
                    if remove {
                        selection.unselect_item(index);
                    }
                }
            }
            guard.set(false);
        });
        let mut columns = BTreeMap::new();
        let order = normalized_order(settings);
        for column_settings in order {
            let Some(kind) = DataColumn::from_id(&column_settings.id) else {
                continue;
            };
            let column = make_column(
                kind,
                column_settings.width,
                column_settings.visible,
                &selection,
                Rc::clone(&handlers),
            );
            view.append_column(&column);
            columns.insert(kind.id().to_owned(), column);
        }

        Self {
            view,
            store,
            selection,
            columns,
            handlers,
        }
    }

    pub(super) fn set_context_handler(
        &self,
        handler: impl Fn(SensorRow, gtk::Widget, f64, f64) + 'static,
    ) {
        self.handlers.borrow_mut().context = Some(Rc::new(handler));
    }

    pub(super) fn sync_rows(&self, rows: &[SensorRow], preserve_ids: &[String]) -> bool {
        if self.has_same_topology(rows) {
            self.update_rows(rows);
            return false;
        }

        self.replace_topology(rows, preserve_ids);
        true
    }

    fn update_rows(&self, rows: &[SensorRow]) {
        for (index, row) in rows.iter().enumerate() {
            let Some(item) = self.store.item(index as u32) else {
                continue;
            };
            let Ok(boxed) = item.downcast::<glib::BoxedAnyObject>() else {
                continue;
            };
            boxed.borrow_mut::<RowState>().update(row);
        }
    }

    fn has_same_topology(&self, rows: &[SensorRow]) -> bool {
        if self.store.n_items() != rows.len() as u32 {
            return false;
        }
        rows.iter().enumerate().all(|(index, row)| {
            let Some(item) = self.store.item(index as u32) else {
                return false;
            };
            let Ok(boxed) = item.downcast::<glib::BoxedAnyObject>() else {
                return false;
            };
            let matches = {
                let stored = boxed.borrow::<RowState>();
                stored.row.id == row.id
            };
            matches
        })
    }

    fn replace_topology(&self, rows: &[SensorRow], preserve_ids: &[String]) {
        let additions = boxed_rows(rows);
        self.store.splice(0, self.store.n_items(), &additions);
        self.selection.unselect_all();
        for id in preserve_ids {
            self.select_id(id, false);
        }
        if preserve_ids.is_empty() && self.store.n_items() > 0 {
            self.selection.select_item(0, true);
        }
    }

    fn select_id(&self, id: &str, unselect_rest: bool) {
        for index in 0..self.store.n_items() {
            let Some(item) = self.store.item(index) else {
                continue;
            };
            let Ok(boxed) = item.downcast::<glib::BoxedAnyObject>() else {
                continue;
            };
            let matches = {
                let row = boxed.borrow::<RowState>();
                row.row.id == id
            };
            if matches {
                self.selection.select_item(index, unselect_rest);
                break;
            }
        }
    }

    pub(super) fn selected_ids(&self) -> Vec<String> {
        self.selected_rows().into_iter().map(|row| row.id).collect()
    }

    pub(super) fn selected_row(&self) -> Option<SensorRow> {
        self.selected_rows().into_iter().next()
    }

    pub(super) fn selected_rows(&self) -> Vec<SensorRow> {
        (0..self.store.n_items())
            .filter(|index| self.selection.is_selected(*index))
            .filter_map(|index| self.row_at(index))
            .collect()
    }

    pub(super) fn row_at(&self, index: u32) -> Option<SensorRow> {
        let item = self.store.item(index)?;
        let boxed = item.downcast::<glib::BoxedAnyObject>().ok()?;
        let row = boxed.borrow::<RowState>().row.clone();
        Some(row)
    }

    pub(super) fn clear_selection(&self) {
        self.selection.unselect_all();
    }

    pub(super) fn set_column_visible(&self, id: &str, visible: bool) {
        if let Some(column) = self.columns.get(id) {
            column.set_visible(visible);
        }
    }

    pub(super) fn column_visible(&self, id: &str) -> bool {
        self.columns
            .get(id)
            .is_some_and(ColumnViewColumn::is_visible)
    }

    pub(super) fn set_density_class(&self, density: Density) {
        for class in ["density-compact", "density-normal", "density-comfortable"] {
            self.view.remove_css_class(class);
        }
        let class = match density {
            Density::Compact => "density-compact",
            Density::Normal => "density-normal",
            Density::Comfortable => "density-comfortable",
        };
        self.view.add_css_class(class);
    }

    pub(super) fn capture_columns(&self) -> Vec<ColumnSettings> {
        let model = self.view.columns();
        let mut settings = Vec::new();
        for index in 0..model.n_items() {
            let Some(item) = model.item(index) else {
                continue;
            };
            let Ok(column) = item.downcast::<ColumnViewColumn>() else {
                continue;
            };
            let Some(id) = self
                .columns
                .iter()
                .find_map(|(id, known)| (known == &column).then(|| id.clone()))
            else {
                continue;
            };
            settings.push(ColumnSettings {
                id,
                width: column.fixed_width().max(72),
                visible: column.is_visible(),
            });
        }
        settings
    }
}

fn boxed_rows(rows: &[SensorRow]) -> Vec<glib::Object> {
    rows.iter()
        .cloned()
        .map(RowState::new)
        .map(|row| glib::BoxedAnyObject::new(row).upcast())
        .collect()
}

fn normalized_order(settings: &[ColumnSettings]) -> Vec<ColumnSettings> {
    let defaults = ColumnSettings::default_layout();
    let mut result = Vec::new();
    for configured in settings {
        if DataColumn::from_id(&configured.id).is_some()
            && !result
                .iter()
                .any(|item: &ColumnSettings| item.id == configured.id)
        {
            result.push(configured.clone());
        }
    }
    for default in defaults {
        if !result.iter().any(|item| item.id == default.id) {
            result.push(default);
        }
    }
    result
}

fn make_column(
    kind: DataColumn,
    width: i32,
    visible: bool,
    selection: &MultiSelection,
    handlers: Rc<RefCell<TableHandlers>>,
) -> ColumnViewColumn {
    let factory = SignalListItemFactory::new();
    let selection_for_setup = selection.clone();
    factory.connect_setup(move |_, object| {
        let Some(item) = object.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let label = Label::new(None);
        label.set_xalign(if kind.numeric() { 1.0 } else { 0.0 });
        label.set_yalign(0.5);
        label.set_hexpand(true);
        label.set_single_line_mode(true);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.add_css_class("sensor-cell");
        if kind.numeric() {
            label.add_css_class("numeric-cell");
        }

        let click = gtk::GestureClick::new();
        click.set_button(3);
        let weak_item = item.downgrade();
        let selection = selection_for_setup.clone();
        let handlers = Rc::clone(&handlers);
        click.connect_pressed(move |gesture, _, x, y| {
            let Some(item) = weak_item.upgrade() else {
                return;
            };
            let Some(boxed) = item.item().and_downcast::<glib::BoxedAnyObject>() else {
                return;
            };
            let row = boxed.borrow::<RowState>().row.clone();
            let position = item.position();
            let preserve_multi = row.kind == RowKind::Sensor
                && selection.is_selected(position)
                && (0..selection.n_items())
                    .filter(|index| selection.is_selected(*index))
                    .filter_map(|index| selection.item(index))
                    .filter_map(|item| item.downcast::<glib::BoxedAnyObject>().ok())
                    .all(|item| item.borrow::<RowState>().row.kind == RowKind::Sensor);
            if !preserve_multi {
                selection.select_item(position, true);
            }
            let Some(widget) = gesture.widget() else {
                return;
            };
            if let Some(handler) = handlers.borrow().context.clone() {
                handler(row, widget, x, y);
            }
        });
        let cell = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        cell.set_hexpand(true);
        cell.add_controller(click);
        if kind == DataColumn::Sensor {
            let prefix = Label::new(None);
            prefix.set_yalign(0.5);
            let slot_text = Label::new(Some(SENSOR_MARKER_SLOT));
            let marker = Label::new(Some("★"));
            marker.set_visible(false);
            marker.set_halign(gtk::Align::Center);
            marker.set_valign(gtk::Align::Center);
            let slot = gtk::Overlay::new();
            slot.set_child(Some(&slot_text));
            slot.add_overlay(&marker);
            cell.append(&prefix);
            cell.append(&slot);
        }
        cell.append(&label);
        item.set_child(Some(&cell));
    });
    factory.connect_bind(move |_, object| {
        let Some(item) = object.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(label) = list_item_label(item) else {
            return;
        };
        let Some(boxed) = item.item().and_downcast::<glib::BoxedAnyObject>() else {
            return;
        };
        let mut state = boxed.borrow_mut::<RowState>();
        state.bind_label(kind, &label);
        if kind == DataColumn::Sensor {
            let Some((prefix, marker, slot)) = list_item_sensor_prefix(item) else {
                return;
            };
            state.bind_sensor_prefix(&prefix, &marker, &slot);
        }
    });
    factory.connect_unbind(move |_, object| {
        let Some(item) = object.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(boxed) = item.item().and_downcast::<glib::BoxedAnyObject>() else {
            return;
        };
        boxed.borrow_mut::<RowState>().unbind_label(kind);
    });

    let column = ColumnViewColumn::new(Some(kind.title()), Some(factory));
    column.set_resizable(true);
    column.set_fixed_width(width.max(72));
    column.set_visible(visible);
    column.set_expand(kind == DataColumn::Sensor);
    column
}

fn list_item_label(item: &gtk::ListItem) -> Option<Label> {
    item.child()
        .and_downcast::<gtk::Box>()?
        .last_child()
        .and_downcast::<Label>()
}

fn list_item_sensor_prefix(item: &gtk::ListItem) -> Option<(Label, Label, gtk::Overlay)> {
    let cell = item.child().and_downcast::<gtk::Box>()?;
    let prefix = cell.first_child().and_downcast::<Label>()?;
    let slot = prefix.next_sibling().and_downcast::<gtk::Overlay>()?;
    let marker = slot.last_child().and_downcast::<Label>()?;
    Some((prefix, marker, slot))
}

fn render_label(kind: DataColumn, row: &SensorRow, label: &Label) {
    let text = kind.text(row);
    set_label_text(label, text.as_ref(), kind.color(row));
    update_row_classes(row, label);
    for class in ["alarm-cell", "fault-cell", "stale-cell"] {
        label.remove_css_class(class);
    }
    if row.dimmed && matches!(kind, DataColumn::Current | DataColumn::Status) {
        label.add_css_class("stale-cell");
    }
    if kind == DataColumn::Status {
        match row.status.as_str() {
            "Hardware alarm" => label.add_css_class("alarm-cell"),
            "Fault" | "Unavailable" => label.add_css_class("fault-cell"),
            _ => {}
        }
    }
    let tooltip = tooltip_text(kind, row, text.as_ref());
    label.set_tooltip_text(tooltip.as_deref());
}

fn tooltip_text(kind: DataColumn, row: &SensorRow, rendered: &str) -> Option<String> {
    if kind == DataColumn::Sensor && row.alias.is_some() {
        Some(format!("{} (original: {})", row.label, row.original_label))
    } else if kind == DataColumn::Sensor {
        Some(row.label.clone())
    } else if rendered.is_empty() {
        None
    } else {
        Some(rendered.to_owned())
    }
}

fn render_sensor_prefix(row: &SensorRow, prefix: &Label, marker: &Label, slot: &gtk::Overlay) {
    let (text, slot_visible, marker_visible) = sensor_prefix_state(row);
    prefix.set_text(&text);
    slot.set_visible(slot_visible);
    marker.set_visible(marker_visible);
    update_row_classes(row, prefix);
}

fn sensor_prefix_state(row: &SensorRow) -> (String, bool, bool) {
    let indent = "  ".repeat(row.depth as usize);
    match row.kind {
        RowKind::Device | RowKind::Header => (
            format!("{indent}{} ", if row.collapsed { "▸" } else { "▾" }),
            false,
            false,
        ),
        RowKind::Sensor => (
            "  ".repeat(row.depth.saturating_sub(1) as usize),
            true,
            row.favorite,
        ),
    }
}

fn update_row_classes(row: &SensorRow, label: &Label) {
    for class in ["device-cell", "group-cell"] {
        label.remove_css_class(class);
    }
    match row.kind {
        RowKind::Device => label.add_css_class("device-cell"),
        RowKind::Header => label.add_css_class("group-cell"),
        RowKind::Sensor => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sensor_row(favorite: bool) -> SensorRow {
        SensorRow {
            id: "sensor:cpu:0:temp:0".to_owned(),
            device_id: "cpu:0".to_owned(),
            sensor_id: Some("temp:0".to_owned()),
            hide_key: "sensor:cpu:0:temp:0".to_owned(),
            kind: RowKind::Sensor,
            depth: 2,
            label: "Average effective clock".to_owned(),
            alias: None,
            original_label: "Average effective clock".to_owned(),
            current: "4.2 GHz".to_owned(),
            minimum: String::new(),
            maximum: String::new(),
            average: String::new(),
            status: String::new(),
            current_color: None,
            minimum_color: None,
            maximum_color: None,
            average_color: None,
            status_color: None,
            dimmed: false,
            current_sample: true,
            favorite,
            collapsed: false,
        }
    }

    #[test]
    fn favorite_state_does_not_change_sensor_name_position() {
        let normal = sensor_prefix_state(&sensor_row(false));
        let favorite = sensor_prefix_state(&sensor_row(true));

        assert_eq!(normal.0, favorite.0);
        assert_eq!(
            normal.0.len() + SENSOR_MARKER_SLOT.len(),
            "  ".repeat(sensor_row(false).depth as usize).len() + 2
        );
        assert!(normal.1 && favorite.1);
        assert!(!normal.2);
        assert!(favorite.2);
        let row = sensor_row(false);
        assert_eq!(
            DataColumn::Sensor.text(&row).as_ref(),
            "Average effective clock"
        );
    }

    #[test]
    fn sensor_tooltips_do_not_include_row_indentation() {
        let row = sensor_row(true);
        assert_eq!(
            tooltip_text(DataColumn::Sensor, &row, "ignored").as_deref(),
            Some("Average effective clock")
        );
    }
}
