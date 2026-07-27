use crate::ui::set_label_text;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::{ColumnView, ColumnViewColumn, Label, SignalListItemFactory, SingleSelection};
use hwall_app::{ColumnSettings, Density, RowKind, SensorRow};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

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
            Self::Sensor => Cow::Owned(sensor_label(row)),
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
}

impl RowState {
    fn new(row: SensorRow) -> Self {
        Self {
            row,
            labels: std::array::from_fn(|_| None),
        }
    }

    fn bind_label(&mut self, kind: DataColumn, label: &Label) {
        self.labels[kind.index()] = Some(label.downgrade());
        render_label(kind, &self.row, label);
    }

    fn unbind_label(&mut self, kind: DataColumn) {
        self.labels[kind.index()] = None;
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
    }
}

#[derive(Clone)]
pub(super) struct SensorTable {
    pub(super) view: ColumnView,
    store: gio::ListStore,
    selection: SingleSelection,
    columns: BTreeMap<String, ColumnViewColumn>,
    handlers: Rc<RefCell<TableHandlers>>,
}

impl SensorTable {
    pub(super) fn new(settings: &[ColumnSettings]) -> Self {
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = SingleSelection::new(Some(store.clone()));
        // Real topology changes replace the model contents. Do not let GTK
        // choose an unrelated fallback row before HWall restores the stable ID.
        selection.set_autoselect(false);
        let view = ColumnView::new(Some(selection.clone()));
        view.set_reorderable(true);
        view.set_show_row_separators(true);
        view.set_show_column_separators(true);
        view.add_css_class("hwall-table");
        view.add_css_class("data-table");
        view.set_vexpand(true);
        view.set_hexpand(true);

        let handlers = Rc::new(RefCell::new(TableHandlers::default()));
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

    pub(super) fn sync_rows(&self, rows: &[SensorRow], preserve_id: Option<&str>) -> bool {
        if self.has_same_topology(rows) {
            self.update_rows(rows);
            return false;
        }

        self.replace_topology(rows, preserve_id);
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

    fn replace_topology(&self, rows: &[SensorRow], preserve_id: Option<&str>) {
        let additions = boxed_rows(rows);
        self.store.splice(0, self.store.n_items(), &additions);
        self.select_id(preserve_id);
        if preserve_id.is_none()
            && self.selection.selected_item().is_none()
            && self.store.n_items() > 0
        {
            self.selection.set_selected(0);
        }
    }

    fn select_id(&self, id: Option<&str>) {
        let Some(id) = id else {
            return;
        };
        if self.selected_id().as_deref() == Some(id) {
            return;
        }
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
                self.selection.set_selected(index);
                break;
            }
        }
    }

    fn selected_id(&self) -> Option<String> {
        let item = self.selection.selected_item()?;
        let boxed = item.downcast::<glib::BoxedAnyObject>().ok()?;
        let id = {
            let row = boxed.borrow::<RowState>();
            row.row.id.clone()
        };
        Some(id)
    }

    pub(super) fn selected_row(&self) -> Option<SensorRow> {
        let item = self.selection.selected_item()?;
        let boxed = item.downcast::<glib::BoxedAnyObject>().ok()?;
        let row = {
            let borrowed = boxed.borrow::<RowState>();
            borrowed.row.clone()
        };
        Some(row)
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
    selection: &SingleSelection,
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
            selection.set_selected(item.position());
            let Some(boxed) = item.item().and_downcast::<glib::BoxedAnyObject>() else {
                return;
            };
            let row = boxed.borrow::<RowState>().row.clone();
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
        boxed.borrow_mut::<RowState>().bind_label(kind, &label);
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
        .first_child()
        .and_downcast::<Label>()
}

fn render_label(kind: DataColumn, row: &SensorRow, label: &Label) {
    let text = kind.text(row);
    set_label_text(label, text.as_ref(), kind.color(row));
    for class in ["device-cell", "group-cell", "alarm-cell", "fault-cell"] {
        label.remove_css_class(class);
    }
    match row.kind {
        RowKind::Device => label.add_css_class("device-cell"),
        RowKind::Header => label.add_css_class("group-cell"),
        RowKind::Sensor => {}
    }
    if kind == DataColumn::Status {
        match row.status.as_str() {
            "Hardware alarm" => label.add_css_class("alarm-cell"),
            "Fault" | "Unavailable" => label.add_css_class("fault-cell"),
            _ => {}
        }
    }
    let tooltip = if kind == DataColumn::Sensor
        && row.kind == RowKind::Sensor
        && row.label != row.original_label
    {
        Some(format!("{} (original: {})", row.label, row.original_label))
    } else if !text.is_empty() {
        Some(text.into_owned())
    } else {
        None
    };
    label.set_tooltip_text(tooltip.as_deref());
}

fn sensor_label(row: &SensorRow) -> String {
    let indent = "  ".repeat(row.depth as usize);
    let disclosure = match row.kind {
        RowKind::Device | RowKind::Header if row.collapsed => "▸ ",
        RowKind::Device | RowKind::Header => "▾ ",
        RowKind::Sensor => "  ",
    };
    let favorite = if row.favorite { "★ " } else { "" };
    format!("{indent}{disclosure}{favorite}{}", row.label)
}
