//! The Database pane: every saved database as a tree beside tabs of consoles, tables,
//! histories and properties, all drawn from the plugin's views.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::f32::consts::FRAC_PI_2;
use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, radians, AnyElement, App, AppContext, ClickEvent, Context, Div, ElementId, Entity,
    FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Subscription, Transformation, WeakEntity, Window,
};
use serde_json::{json, Value};
use sikemux_plugin_database_views::{
    Connection, Explorer, History, HistoryEntry, Schema, TableEntry, TableGroup, TableKind,
    TableView,
};

use super::{after, listen, Host, Panel, PluginPane};
use crate::icons::Icon;
use crate::theme::{
    self, Tokens, RADIUS_2, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_3H, SPACE_4,
    SPACE_5, TEXT_2XS_H, TEXT_XS_H,
};
use crate::ui::{self, Button, IconButton, Variant};

mod console;
mod form;

pub const REGISTRATION: super::Registration = super::Registration {
    plugin: "sikemux.database",
    surface: "browser",
    pane: super::boxed::<Database>,
    top_bar: None,
    palettes: &[],
};

#[derive(Default)]
pub struct Database {
    /// The explorer's open nodes.
    open: BTreeSet<Node>,
    tabs: Vec<Tab>,
    front: Option<Tab>,
    /// The explorer last read, kept while one with other nodes open is on its way.
    shown: Option<Rc<Explorer>>,
    filter: Option<Entity<InputState>>,
    search: Option<Entity<InputState>>,
    searched: String,
    connecting: HashSet<String>,
    /// Why connecting to each failed, by connection.
    failed: HashMap<String, SharedString>,
    busy: bool,
    /// The connection whose history the person is asked to clear.
    clearing: Option<(String, String)>,
    /// Each console's SQL and results, by its tab's `console_key`.
    consoles: HashMap<String, console::Console>,
    /// The table tabs showing the table's columns and keys rather than its rows.
    structure: HashSet<Tab>,
    /// Runs started, so each is named apart from the others.
    runs: u64,
    /// The connection form, while it shows.
    form: Option<form::Form>,
    /// The form is on its way from the plugin.
    form_loading: bool,
    /// The form the plugin sent, made into fields at the next draw.
    form_arrived: Option<form::Arrived>,
    /// The form this pane's slot left open was looked for.
    form_restored: bool,
    /// The connection the person is asked to remove.
    removing: Option<(String, String)>,
    _typing: Vec<Subscription>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Node {
    Connection(String),
    Schema(String, String),
    Group(String, String, String),
    Table(String, String, String),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Tab {
    Console {
        id: String,
        number: usize,
    },
    Table {
        id: String,
        schema: String,
        name: String,
    },
    History {
        id: String,
    },
    Properties {
        id: String,
    },
}

impl Tab {
    fn connection(&self) -> &str {
        match self {
            Tab::Console { id, .. }
            | Tab::Table { id, .. }
            | Tab::History { id }
            | Tab::Properties { id } => id,
        }
    }

    /// Names the console holding the tab's SQL and results, for a console or a table's data.
    fn console_key(&self) -> Option<String> {
        match self {
            Tab::Console { id, number } => Some(format!("{id}/{number}")),
            Tab::Table { id, schema, name } => Some(format!("{id}/table/{schema}/{name}")),
            Tab::History { .. } | Tab::Properties { .. } => None,
        }
    }

    fn label(&self) -> String {
        match self {
            Tab::Console { number: 1, .. } => "console".into(),
            Tab::Console { number, .. } => format!("console {number}"),
            Tab::Table { name, .. } => name.clone(),
            Tab::History { .. } => "history".into(),
            Tab::Properties { .. } => "properties".into(),
        }
    }

    fn icon(&self) -> Icon {
        match self {
            Tab::Console { .. } => Icon::Code,
            Tab::Table { .. } => Icon::Table,
            Tab::History { .. } => Icon::Clock,
            Tab::Properties { .. } => Icon::Settings,
        }
    }
}

impl Panel for Database {
    fn render(
        &mut self,
        host: &mut Host,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        self.fields(window, cx);
        if !std::mem::replace(&mut self.form_restored, true) {
            self.form = form::restore(host, window, cx);
        }
        if let Some(arrived) = self.form_arrived.take() {
            self.form = Some(arrived.into_form(window, cx));
        }
        form::keep(host, self.form.as_ref(), cx);
        let read = host
            .view("explorerView", json!({ "open": self.open_nodes() }), cx)
            .read::<Explorer>();
        match read {
            Ok(Some(explorer)) => self.shown = Some(explorer),
            Err(failure) if self.shown.is_none() => return super::failure(&failure, cx),
            _ => {}
        }
        let Some(explorer) = self.shown.clone() else {
            return super::loading(cx);
        };
        if self.form.is_some() || self.form_loading {
            return div()
                .size_full()
                .child(form::page(self, cx))
                .children(self.asking(cx))
                .into_any_element();
        }
        if explorer.connections.is_empty() {
            return welcome(cx);
        }
        let main = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .children((!self.tabs.is_empty()).then(|| self.strip(&explorer, cx)))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.body(&explorer, host, window, cx)),
            );
        div()
            .size_full()
            .flex()
            .child(self.explorer(&explorer, cx))
            .child(main)
            .children(self.asking(cx))
            .into_any_element()
    }

    /// `at` is a connection an agent proposed, to finish in a new connection's form.
    fn open_at(&mut self, at: &str, host: &mut Host, cx: &mut Context<PluginPane>) {
        if let Ok(proposed) = serde_json::from_str(at) {
            form::open(self, host, None, Some(proposed), cx);
        }
    }
}

impl Database {
    fn fields(&mut self, window: &mut Window, cx: &mut Context<PluginPane>) {
        if self.filter.is_some() {
            return;
        }
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter tables"));
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search past queries"));
        self._typing
            .push(cx.subscribe(&filter, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }));
        self._typing.push(
            cx.subscribe(&search, |pane, search, event: &InputEvent, cx| {
                if !matches!(event, InputEvent::Change) {
                    return;
                }
                let typed = search.read(cx).value().trim().to_owned();
                if let Some((database, _)) = pane.parts::<Database>() {
                    database.searched = typed;
                }
                cx.notify();
            }),
        );
        self.filter = Some(filter);
        self.search = Some(search);
    }

    /// What the explorer view reads: in each open connection, its open schemas and the open
    /// tables in them.
    fn open_nodes(&self) -> Vec<Value> {
        let mut asked: BTreeMap<&str, (Vec<&str>, Vec<Value>)> = BTreeMap::new();
        for node in &self.open {
            if let Node::Connection(id) = node {
                asked.entry(id).or_default();
            }
        }
        for node in &self.open {
            match node {
                Node::Schema(id, schema) => {
                    if let Some((schemas, _)) = asked.get_mut(id.as_str()) {
                        schemas.push(schema);
                    }
                }
                Node::Table(id, schema, name)
                    if self
                        .open
                        .contains(&Node::Schema(id.clone(), schema.clone())) =>
                {
                    if let Some((_, tables)) = asked.get_mut(id.as_str()) {
                        tables.push(json!({ "schema": schema, "name": name }));
                    }
                }
                _ => {}
            }
        }
        asked
            .into_iter()
            .map(
                |(id, (schemas, tables))| json!({ "id": id, "schemas": schemas, "tables": tables }),
            )
            .collect()
    }

    fn toggle(&mut self, node: Node) {
        if !self.open.remove(&node) {
            self.open.insert(node);
        }
    }

    /// Shows the tab, opening it just after the one in front when it is not open yet.
    fn show(&mut self, tab: Tab) {
        if !self.tabs.contains(&tab) {
            let at = self
                .front
                .as_ref()
                .and_then(|front| self.tabs.iter().position(|each| each == front))
                .map_or(self.tabs.len(), |at| at + 1);
            self.tabs.insert(at, tab.clone());
        }
        self.front = Some(tab);
    }

    /// Closes the tab; when it was in front, its right-hand neighbour takes its place, or else
    /// its left one.
    fn close(&mut self, tab: &Tab) {
        let Some(at) = self.tabs.iter().position(|each| each == tab) else {
            return;
        };
        self.tabs.remove(at);
        if let Some(key) = tab.console_key() {
            self.consoles.remove(&key);
        }
        self.structure.remove(tab);
        if self.front.as_ref() == Some(tab) {
            self.front = self
                .tabs
                .get(at)
                .or_else(|| at.checked_sub(1).and_then(|left| self.tabs.get(left)))
                .cloned();
        }
    }

    fn connect(&mut self, host: &mut Host, connection: &Connection, cx: &mut Context<PluginPane>) {
        let id = connection.id.clone();
        if !self.connecting.insert(id.clone()) {
            return;
        }
        self.failed.remove(&id);
        let usual = connection.default_schema.clone();
        let call = host.call("connect", json!({ "id": id }));
        after::<Database, _>(
            call,
            move |database, _, done, _| {
                database.connecting.remove(&id);
                match done {
                    Ok(_) => {
                        if database.tabs.is_empty() {
                            database.show(Tab::Console {
                                id: id.clone(),
                                number: 1,
                            });
                        }
                        database.open.insert(Node::Connection(id.clone()));
                        if !usual.is_empty() {
                            database
                                .open
                                .insert(Node::Schema(id.clone(), usual.clone()));
                            database
                                .open
                                .insert(Node::Group(id, usual, "tables".into()));
                        }
                    }
                    Err(failure) => {
                        database.failed.insert(id, failure.message);
                    }
                }
            },
            cx,
        );
        cx.notify();
    }

    /// A console for the connection: the one in front if it is one, else its first, else a
    /// new one.
    fn console_for(&self, id: &str) -> Tab {
        let consoles: Vec<&Tab> = self
            .tabs
            .iter()
            .filter(|tab| matches!(tab, Tab::Console { id: of, .. } if of == id))
            .collect();
        consoles
            .iter()
            .find(|tab| self.front.as_ref() == Some(**tab))
            .or(consoles.first())
            .map_or_else(
                || Tab::Console {
                    id: id.to_owned(),
                    number: 1,
                },
                |tab| (*tab).clone(),
            )
    }

    fn new_console(&self, id: &str) -> Tab {
        let last = self
            .tabs
            .iter()
            .filter_map(|tab| match tab {
                Tab::Console { id: of, number } if of == id => Some(*number),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        Tab::Console {
            id: id.to_owned(),
            number: last + 1,
        }
    }

    /// The console's state, made the first time it shows.
    fn console(&mut self, key: &str, id: &str, window: &mut Window, cx: &mut Context<PluginPane>) {
        if !self.consoles.contains_key(key) {
            let console = console::Console::new(key.to_owned(), id.to_owned(), false, window, cx);
            self.consoles.insert(key.to_owned(), console);
        }
    }

    /// Puts SQL in a console of the connection, to run when the person is ready.
    fn query(&mut self, id: &str, sql: &str, window: &mut Window, cx: &mut Context<PluginPane>) {
        let tab = self.console_for(id);
        let Tab::Console { number, .. } = &tab else {
            return;
        };
        let key = format!("{id}/{number}");
        self.show(tab.clone());
        self.console(&key, id, window, cx);
        if let Some(console) = self.consoles.get(&key) {
            console.load(sql, window, cx);
        }
    }

    /// Calls the plugin's `method`, saying over the window when it fails; the core reads the
    /// views again after it.
    fn write(
        &mut self,
        host: &Host,
        method: &'static str,
        params: Value,
        cx: &mut Context<PluginPane>,
    ) {
        self.busy = true;
        let call = host.call(method, params);
        after::<Database, _>(
            call,
            |database, host, done, cx| {
                database.busy = false;
                if let Err(failure) = done {
                    host.said(&failure, cx);
                }
            },
            cx,
        );
        cx.notify();
    }

    fn explorer(&self, explorer: &Explorer, cx: &mut Context<PluginPane>) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let words = self
            .filter
            .as_ref()
            .map(|filter| filter.read(cx).value().trim().to_lowercase())
            .unwrap_or_default();
        let mut rows = Vec::new();
        for connection in &explorer.connections {
            self.connection_rows(connection, &words, false, &mut rows, cx);
        }
        let head = div()
            .h(px(36.))
            .px(SPACE_2H)
            .flex()
            .items_center()
            .gap(SPACE_1)
            .child(ui::caps("Explorer", cx))
            .child(div().flex_1())
            .child(
                IconButton::new("db-refresh", Icon::Refresh, "Refresh")
                    .small()
                    .on_click(listen::<Database, _>(cx, |_, host, _, _, _| {
                        host.read_again(None)
                    })),
            )
            .child(
                IconButton::new("db-collapse", Icon::CollapseAll, "Collapse all")
                    .small()
                    .on_click(listen::<Database, _>(cx, |database, _, _, _, cx| {
                        database.open.clear();
                        cx.notify();
                    })),
            )
            .child(
                IconButton::new("db-new-connection", Icon::Plus, "New connection")
                    .small()
                    .on_click(listen::<Database, _>(cx, |database, host, _, _, cx| {
                        form::open(database, host, None, None, cx)
                    })),
            );
        div()
            .w(px(280.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(t.border)
            .child(head)
            .children(
                self.filter
                    .as_ref()
                    .map(|filter| ui::Field::new(filter).icon(Icon::Search).flush()),
            )
            .child(
                div()
                    .id("db-explorer")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(SPACE_1)
                    .flex()
                    .flex_col()
                    .children(rows),
            )
            .into_any_element()
    }

    /// The connection's row and, while it is open, its schemas or, on a whole server, its
    /// databases.
    fn connection_rows(
        &self,
        connection: &Connection,
        words: &str,
        nested: bool,
        rows: &mut Vec<AnyElement>,
        cx: &mut Context<PluginPane>,
    ) {
        let t = Tokens::get(cx).clone();
        let id = connection.id.clone();
        let connected = connection.connected.is_some();
        let open = connected && self.open.contains(&Node::Connection(id.clone()));
        let detail = match (self.connecting.contains(&id), connected) {
            (true, _) => Some(ui::meta("connecting…", cx).into_any_element()),
            (false, true) => Some(ui::dot(ui::Tone::Ok, cx).into_any_element()),
            (false, false) => None,
        };
        let count = open
            .then(|| {
                (connection.schemas.as_ref().map(Vec::len))
                    .or(connection.databases.as_ref().map(Vec::len))
                    .map(|count| count.to_string())
            })
            .flatten();
        let picked = connection.clone();
        let row = node(
            format!("db-connection/{id}"),
            0,
            Some(open),
            false,
            Some((Icon::Database, Tokens::get(cx).text_tertiary)),
            connection.name.clone(),
            cx,
        )
        .children(count.map(|count| ui::meta(count, cx)))
        .children(detail)
        .tooltip(ui::tooltip(format!(
            "{} · {}",
            connection.engine_name, connection.address
        )))
        .on_click(listen::<Database, _>(
            cx,
            move |database, host, _, _, cx| {
                match picked.connected {
                    Some(_) => database.toggle(Node::Connection(picked.id.clone())),
                    None => database.connect(host, &picked, cx),
                }
                cx.notify();
            },
        ))
        .context_menu(ui::menu(connection_menu(
            connection,
            nested,
            cx.weak_entity(),
        )));
        rows.push(row.into_any_element());
        if let Some(failed) = self.failed.get(&id) {
            rows.push(note(1, failed.clone(), true, &t));
        }
        if !open {
            return;
        }
        match (
            &connection.failed,
            &connection.schemas,
            &connection.databases,
        ) {
            (Some(failed), _, _) => rows.push(note(1, failed.clone().into(), true, &t)),
            (None, Some(schemas), _) => {
                for schema in schemas {
                    self.schema_rows(&id, schema, words, rows, cx);
                }
            }
            (None, None, Some(databases)) => {
                let mut nested = Vec::new();
                for database in databases {
                    self.connection_rows(database, words, true, &mut nested, cx);
                }
                rows.extend(
                    (nested.into_iter())
                        .map(|row| div().pl(SPACE_3H).child(row).into_any_element()),
                );
            }
            (None, None, None) => rows.push(note(1, "Loading…".into(), false, &t)),
        }
    }

    fn schema_rows(
        &self,
        id: &str,
        schema: &Schema,
        words: &str,
        rows: &mut Vec<AnyElement>,
        cx: &mut Context<PluginPane>,
    ) {
        let t = Tokens::get(cx).clone();
        let key = Node::Schema(id.to_owned(), schema.name.clone());
        let open = self.open.contains(&key);
        let count = open
            .then(|| {
                schema.groups.as_ref().map(|groups| {
                    groups
                        .iter()
                        .map(|group| group.tables.len())
                        .sum::<usize>()
                        .to_string()
                })
            })
            .flatten();
        rows.push(
            node(
                format!("db-schema/{id}/{}", schema.name),
                1,
                Some(open),
                false,
                Some((Icon::Stack, Tokens::get(cx).warn)),
                schema.name.clone(),
                cx,
            )
            .children(count.map(|count| ui::meta(count, cx)))
            .on_click(listen::<Database, _>(cx, move |database, _, _, _, cx| {
                database.toggle(key.clone());
                cx.notify();
            }))
            .context_menu(ui::menu(schema_menu(id, &schema.name, cx.weak_entity())))
            .into_any_element(),
        );
        if !open {
            return;
        }
        match (&schema.failed, &schema.groups) {
            (Some(failed), _) => rows.push(note(2, failed.clone().into(), true, &t)),
            (None, None) => rows.push(note(2, "Loading tables…".into(), false, &t)),
            (None, Some(groups)) if groups.is_empty() => {
                rows.push(note(2, "No tables here yet.".into(), false, &t))
            }
            (None, Some(groups)) => {
                for group in groups {
                    self.group_rows(id, &schema.name, group, words, rows, cx);
                }
            }
        }
    }

    fn group_rows(
        &self,
        id: &str,
        schema: &str,
        group: &TableGroup,
        words: &str,
        rows: &mut Vec<AnyElement>,
        cx: &mut Context<PluginPane>,
    ) {
        let key = Node::Group(id.to_owned(), schema.to_owned(), group.name.clone());
        let open = self.open.contains(&key);
        let members: Vec<&TableEntry> = group
            .tables
            .iter()
            .filter(|table| words.is_empty() || table.name.to_lowercase().contains(words))
            .collect();
        let total = group.tables.len();
        let count = match members.len() == total {
            true => total.to_string(),
            false => format!("{} of {total}", members.len()),
        };
        rows.push(
            node(
                format!("db-group/{id}/{schema}/{}", group.name),
                2,
                Some(open),
                false,
                Some((Icon::Folder, Tokens::get(cx).live)),
                group.name.clone(),
                cx,
            )
            .child(ui::meta(count, cx))
            .on_click(listen::<Database, _>(cx, move |database, _, _, _, cx| {
                database.toggle(key.clone());
                cx.notify();
            }))
            .into_any_element(),
        );
        if open {
            for table in members {
                self.table_rows(id, schema, table, rows, cx);
            }
        }
    }

    fn table_rows(
        &self,
        id: &str,
        schema: &str,
        table: &TableEntry,
        rows: &mut Vec<AnyElement>,
        cx: &mut Context<PluginPane>,
    ) {
        let t = Tokens::get(cx).clone();
        let key = Node::Table(id.to_owned(), schema.to_owned(), table.name.clone());
        let open = self.open.contains(&key);
        let tab = Tab::Table {
            id: id.to_owned(),
            schema: schema.to_owned(),
            name: table.name.clone(),
        };
        let current = self.front.as_ref() == Some(&tab);
        let icon = match table.kind {
            TableKind::Table | TableKind::ForeignTable => (Icon::Table, t.git_renamed),
            TableKind::View | TableKind::MaterializedView => (Icon::Eye, t.cmd),
        };
        let opening = tab.clone();
        let pane = cx.weak_entity();
        let (name, qualified) = (table.name.clone(), format!("{schema}.{}", table.name));
        let toggled = key.clone();
        let (connection_id, preview) = (id.to_owned(), table.preview.clone());
        let structure_tab = tab.clone();
        rows.push(
            node(
                format!("db-table/{id}/{schema}/{}", table.name),
                3,
                Some(open),
                current,
                Some(icon),
                table.name.clone(),
                cx,
            )
            .tooltip(ui::tooltip(format!("{qualified}, a {}", table.kind_name)))
            .on_click(listen::<Database, _>(cx, move |database, _, _, _, cx| {
                database.show(opening.clone());
                cx.notify();
            }))
            .context_menu(ui::menu(move |popup: PopupMenu, _, _| {
                let (show, columns) = (tab.clone(), toggled.clone());
                let structure = structure_tab.clone();
                let (query_id, query_sql) = (connection_id.clone(), preview.clone());
                popup
                    .item(
                        PopupMenuItem::new("Open data")
                            .icon(Icon::Table)
                            .on_click(on(&pane, move |database, _, _, _| {
                                database.structure.remove(&show);
                                database.show(show.clone())
                            })),
                    )
                    .item(
                        PopupMenuItem::new("Open structure")
                            .icon(Icon::InfoCircle)
                            .on_click(on(&pane, move |database, _, _, _| {
                                database.structure.insert(structure.clone());
                                database.show(structure.clone())
                            })),
                    )
                    .item(
                        PopupMenuItem::new("Query this table")
                            .icon(Icon::Code)
                            .on_click(on(&pane, move |database, _, window, cx| {
                                database.query(&query_id, &query_sql, window, cx)
                            })),
                    )
                    .item(
                        PopupMenuItem::new(if open { "Hide columns" } else { "Show columns" })
                            .icon(Icon::Chevron)
                            .on_click(on(&pane, move |database, _, _, _| {
                                database.toggle(columns.clone())
                            })),
                    )
                    .separator()
                    .item(copy_item(
                        "Copy name",
                        name.clone(),
                        "the table name",
                        &pane,
                    ))
                    .item(copy_item(
                        "Copy qualified name",
                        qualified.clone(),
                        "the qualified name",
                        &pane,
                    ))
            }))
            .into_any_element(),
        );
        if !open {
            return;
        }
        match (&table.failed, &table.columns) {
            (Some(failed), _) => rows.push(note(4, failed.clone().into(), true, &t)),
            (None, None) => rows.push(note(4, "Loading columns…".into(), false, &t)),
            (None, Some(columns)) => {
                for column in columns {
                    let mark = column
                        .primary_key
                        .then(|| ui::caps("PK", cx).text_color(t.warn).into_any_element());
                    let detail = div()
                        .flex()
                        .items_center()
                        .gap(SPACE_1H)
                        .children(mark)
                        .child(
                            ui::mono(column.type_name.clone(), 10.5, cx)
                                .text_color(t.text_tertiary),
                        )
                        .into_any_element();
                    let column_id =
                        format!("db-column/{id}/{schema}/{}/{}", table.name, column.name);
                    let menu = (cx.weak_entity(), column.name.clone());
                    rows.push(
                        node(column_id, 4, None, false, None, column.name.clone(), cx)
                            .child(detail)
                            .context_menu(ui::menu(move |popup: PopupMenu, _, _| {
                                popup.item(copy_item(
                                    "Copy name",
                                    menu.1.clone(),
                                    "the column name",
                                    &menu.0,
                                ))
                            }))
                            .into_any_element(),
                    );
                }
            }
        }
    }

    fn strip(&self, explorer: &Explorer, cx: &mut Context<PluginPane>) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let name_of = |id: &str| {
            connection_of(&explorer.connections, id)
                .map_or("removed".to_owned(), |connection| connection.name.clone())
        };
        div()
            .id("db-tabs")
            .h(px(36.))
            .px(SPACE_1H)
            .flex()
            .items_center()
            .gap(SPACE_1)
            .overflow_x_scroll()
            .border_b_1()
            .border_color(t.border)
            .children(self.tabs.iter().enumerate().map(|(at, tab)| {
                let (picked, closed) = (tab.clone(), tab.clone());
                let menu = tab_menu(tab, self.tabs.len() < 2, cx.weak_entity());
                let tab = ui::Tab::new(
                    ("db-tab", at),
                    format!("{} {}", name_of(tab.connection()), tab.label()),
                )
                .mark(ui::icon(tab.icon(), 12.).text_color(t.text_tertiary))
                .active(self.front.as_ref() == Some(tab))
                .on_click(listen::<Database, _>(cx, move |database, _, _, _, cx| {
                    database.front = Some(picked.clone());
                    cx.notify();
                }))
                .on_close(listen::<Database, _>(
                    cx,
                    move |database, _, _, _, cx| {
                        database.close(&closed);
                        cx.notify();
                    },
                ));
                div()
                    .id(("db-tab-menu", at))
                    .child(tab)
                    .context_menu(ui::menu(menu))
            }))
            .into_any_element()
    }

    fn body(
        &mut self,
        explorer: &Explorer,
        host: &mut Host,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let Some(tab) = self.front.clone() else {
            return super::empty(
                "Open a connection in the explorer to browse its tables.",
                cx,
            );
        };
        let Some(connection) = connection_of(&explorer.connections, tab.connection()) else {
            return super::empty("This connection was removed.", cx);
        };
        match &tab {
            Tab::History { id } => self.history(id, connection, host, cx),
            Tab::Console { .. } | Tab::Table { .. } | Tab::Properties { .. }
                if connection.connected.is_none() =>
            {
                self.properties(connection, cx)
            }
            Tab::Console { id, number } => {
                let key = format!("{id}/{number}");
                self.console(&key, id, window, cx);
                console::page(self, &key, connection, None, host, cx)
            }
            Tab::Properties { .. } => self.properties(connection, cx),
            Tab::Table { id, schema, name } => {
                self.table(&tab, id, schema, name, connection, host, window, cx)
            }
        }
    }

    /// A table's rows, read with its preview query as soon as it opens, or its structure.
    #[allow(clippy::too_many_arguments)]
    fn table(
        &mut self,
        tab: &Tab,
        id: &str,
        schema: &str,
        name: &str,
        connection: &Connection,
        host: &mut Host,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let read = host
            .view(
                "tableView",
                json!({ "id": id, "schema": schema, "table": name }),
                cx,
            )
            .read::<TableView>();
        let info = match read {
            Ok(None) => return super::loading(cx),
            Err(failure) => return super::failure(&failure, cx),
            Ok(Some(info)) => info,
        };
        let structure = self.structure.contains(tab);
        let switch = table_switch(tab, structure, cx);
        if structure {
            return table_page(id, &info, switch, cx);
        }
        let Some(key) = tab.console_key() else {
            return super::loading(cx);
        };
        if !self.consoles.contains_key(&key) {
            let console = console::Console::new(key.clone(), id.to_owned(), true, window, cx);
            console.load(&info.preview, window, cx);
            self.consoles.insert(key.clone(), console);
            console::run(self, host, &key, cx);
        }
        console::page(self, &key, connection, Some(switch), host, cx)
    }

    fn properties(&self, connection: &Connection, cx: &mut Context<PluginPane>) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let connected = connection.connected.is_some();
        let label = match (self.busy, connected) {
            (true, true) => "Disconnecting…",
            (true, false) => "Connecting…",
            (false, true) => "Disconnect",
            (false, false) => "Connect",
        };
        let picked = connection.clone();
        let facts = connection.facts.iter().map(|fact| {
            let value = match fact.mono {
                true => ui::mono(fact.value.clone(), 11.5, cx),
                false => ui::text(fact.value.clone(), 12.5, FontWeight::NORMAL),
            };
            div()
                .flex()
                .gap(SPACE_4)
                .child(
                    div()
                        .w(px(90.))
                        .flex_none()
                        .child(ui::meta(fact.label.clone(), cx)),
                )
                .child(div().min_w_0().text_color(t.text_primary).child(value))
        });
        div()
            .id("db-properties")
            .size_full()
            .overflow_y_scroll()
            .px(SPACE_5)
            .py(SPACE_4)
            .flex()
            .flex_col()
            .gap(SPACE_3H)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .child(
                        ui::text(connection.name.clone(), 15., FontWeight::SEMIBOLD)
                            .text_color(t.text_primary),
                    )
                    .children(connection.read_only.then(|| ui::caps("Read only", cx))),
            )
            .child(div().flex().flex_col().gap(SPACE_1H).children(facts))
            .child(
                div().flex().child(
                    Button::new(
                        "db-connect",
                        label,
                        if connected {
                            Variant::Outline
                        } else {
                            Variant::Primary
                        },
                    )
                    .disabled(self.busy)
                    .on_click(listen::<Database, _>(
                        cx,
                        move |database, host, _, _, cx| match picked.connected {
                            Some(_) => {
                                database.write(host, "disconnect", json!({ "id": picked.id }), cx)
                            }
                            None => database.connect(host, &picked, cx),
                        },
                    )),
                ),
            )
            .into_any_element()
    }

    fn history(
        &self,
        id: &str,
        connection: &Connection,
        host: &mut Host,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let read = host
            .view(
                "historyView",
                json!({ "id": id, "search": self.searched }),
                cx,
            )
            .read::<History>();
        let has_entries = matches!(&read, Ok(Some(history)) if !history.entries.is_empty());
        let (clear_id, clear_name) = (id.to_owned(), connection.name.clone());
        let toolbar = div()
            .px(SPACE_2H)
            .py(SPACE_2)
            .flex()
            .items_center()
            .gap(SPACE_2)
            .children(self.search.as_ref().map(|search| {
                div()
                    .flex_1()
                    .child(ui::Field::new(search).icon(Icon::Search))
            }))
            .child(
                Button::new("db-clear-history", "Clear history", Variant::Outline)
                    .disabled(!has_entries)
                    .on_click(listen::<Database, _>(cx, move |database, _, _, _, cx| {
                        database.clearing = Some((clear_id.clone(), clear_name.clone()));
                        cx.notify();
                    })),
            );
        let body = match read {
            Ok(None) => super::loading(cx),
            Err(failure) => super::failure(&failure, cx),
            Ok(Some(history)) if history.entries.is_empty() => {
                super::empty(history.empty.clone(), cx)
            }
            Ok(Some(history)) => div()
                .id("db-history")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px(SPACE_2H)
                .pb(SPACE_2H)
                .flex()
                .flex_col()
                .gap(SPACE_1H)
                .children(history.entries.iter().map(|entry| history_row(entry, cx)))
                .into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(toolbar)
            .child(body)
            .into_any_element()
    }

    fn asking(&self, cx: &mut Context<PluginPane>) -> Option<AnyElement> {
        if let Some((id, name)) = self.removing.clone() {
            return Some(self.removal(id, name, cx));
        }
        let (id, name) = self.clearing.clone()?;
        let (yes, no) = (cx.weak_entity(), cx.weak_entity());
        Some(
            ui::Confirm::new(
                format!("Clear the history of {name}?"),
                "Every query run here, by you or an agent, is forgotten.",
                "Clear",
            )
            .marked()
            .destructive(true)
            .on_yes(move |_, cx| {
                let id = id.clone();
                let _ = yes.update(cx, |pane, cx| {
                    if let Some((database, host)) = pane.parts::<Database>() {
                        database.clearing = None;
                        database.write(host, "clearHistory", json!({ "id": id }), cx);
                    }
                });
            })
            .on_no(move |_, cx| {
                let _ = no.update(cx, |pane, cx| {
                    if let Some((database, _)) = pane.parts::<Database>() {
                        database.clearing = None;
                    }
                    cx.notify();
                });
            })
            .into_any_element(),
        )
    }
}

impl Database {
    fn removal(&self, id: String, name: String, cx: &mut Context<PluginPane>) -> AnyElement {
        let (yes, no) = (cx.weak_entity(), cx.weak_entity());
        ui::Confirm::new(
            format!("Remove {name}?"),
            "Sikemux forgets this connection and its saved password. The database itself is not touched.",
            "Remove",
        )
        .marked()
        .destructive(true)
        .on_yes(move |_, cx| {
            let id = id.clone();
            let _ = yes.update(cx, |pane, cx| {
                if let Some((database, host)) = pane.parts::<Database>() {
                    database.removing = None;
                    let call = host.call("remove", json!({ "id": id }));
                    after::<Database, _>(
                        call,
                        move |database, host, done, cx| match done {
                            Ok(_) => {
                                database.form = None;
                                let closed: Vec<Tab> = database
                                    .tabs
                                    .iter()
                                    .filter(|tab| tab.connection() == id)
                                    .cloned()
                                    .collect();
                                for tab in &closed {
                                    database.close(tab);
                                }
                            }
                            Err(failure) => host.said(&failure, cx),
                        },
                        cx,
                    );
                }
                cx.notify();
            });
        })
        .on_no(move |_, cx| {
            let _ = no.update(cx, |pane, cx| {
                if let Some((database, _)) = pane.parts::<Database>() {
                    database.removing = None;
                }
                cx.notify();
            });
        })
        .into_any_element()
    }
}

/// What the pane says while no database is saved, with the way to save one.
fn welcome(cx: &mut Context<PluginPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(SPACE_2H)
        .child(ui::icon(Icon::Database, 28.).text_color(t.text_tertiary))
        .child(ui::text("Connect a database", 15., FontWeight::SEMIBOLD).text_color(t.text_primary))
        .child(
            div()
                .max_w(px(420.))
                .text_center()
                .text_size(theme::ui_text(TEXT_XS_H))
                .text_color(t.text_secondary)
                .child("Save a PostgreSQL or MySQL server, or a SQLite file, to browse its tables and run SQL beside your code. Your agents can use it too."),
        )
        .child(
            Button::new("db-welcome-add", "Add a connection", Variant::Primary).on_click(
                listen::<Database, _>(cx, |database, host, _, _, cx| {
                    form::open(database, host, None, None, cx)
                }),
            ),
        )
        .child(ui::meta("Passwords stay in the Keychain.", cx))
        .into_any_element()
}

/// A table's columns, indexes and foreign keys; a foreign key opens the table it points at.
/// Switches a table tab between its rows and its structure.
fn table_switch(tab: &Tab, structure: bool, cx: &mut Context<PluginPane>) -> AnyElement {
    let (pane, picked_tab) = (cx.weak_entity(), tab.clone());
    let key = tab.console_key().unwrap_or_default();
    ui::Segmented::words(
        SharedString::from(format!("db-table-view-{key}")),
        vec![("Data".into(), None), ("Structure".into(), None)],
        usize::from(structure),
    )
    .slide()
    .on_select(move |picked, _, cx| {
        let (picked, tab) = (*picked, picked_tab.clone());
        let _ = pane.update(cx, |pane, cx| {
            if let Some((database, _)) = pane.parts::<Database>() {
                if picked == 1 {
                    database.structure.insert(tab);
                } else {
                    database.structure.remove(&tab);
                }
            }
            cx.notify();
        });
    })
    .into_any_element()
}

fn table_page(
    id: &str,
    info: &TableView,
    switch: AnyElement,
    cx: &mut Context<PluginPane>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let heading = |text: &'static str| {
        ui::text(text, 12., FontWeight::SEMIBOLD)
            .text_color(t.text_secondary)
            .mt(SPACE_1H)
    };
    let cell = |share: Option<f32>| {
        let cell = div().min_w_0().truncate();
        match share {
            Some(share) => cell.w(gpui_kit::relative(share)).flex_none(),
            None => cell.flex_1(),
        }
    };
    let header = div()
        .w_full()
        .flex()
        .gap(SPACE_3)
        .py(SPACE_1)
        .border_b_1()
        .border_color(t.border)
        .child(cell(Some(0.3)).child(ui::meta("Name", cx)))
        .child(cell(Some(0.24)).child(ui::meta("Type", cx)))
        .child(cell(Some(0.07)).child(ui::meta("Null", cx)))
        .child(cell(None).child(ui::meta("Default", cx)));
    let columns = info.columns.iter().map(|column| {
        div()
            .w_full()
            .flex()
            .items_center()
            .gap(SPACE_3)
            .py(px(3.))
            .child(
                cell(Some(0.3))
                    .flex()
                    .items_center()
                    .gap(SPACE_1H)
                    .child(ui::mono(column.name.clone(), 11.5, cx).text_color(t.text_primary))
                    .children(column.primary_key.then(|| ui::caps("PK", cx))),
            )
            .child(cell(Some(0.24)).child(ui::mono(column.type_name.clone(), 11.5, cx)))
            .child(cell(Some(0.07)).child(ui::meta(if column.nullable { "yes" } else { "no" }, cx)))
            .child(
                cell(None).child(
                    ui::mono(column.default.clone().unwrap_or_default(), 11.5, cx)
                        .text_color(t.text_tertiary),
                ),
            )
    });
    let indexes = (!info.indexes.is_empty()).then(|| {
        div()
            .flex()
            .flex_col()
            .gap(px(3.))
            .child(heading("Indexes"))
            .children(info.indexes.iter().map(|index| {
                div()
                    .flex()
                    .gap(SPACE_2)
                    .child(ui::mono(index.name.clone(), 11.5, cx).text_color(t.text_primary))
                    .child(ui::meta(index.detail.clone(), cx))
            }))
    });
    let keys = (!info.foreign_keys.is_empty()).then(|| {
        div()
            .flex()
            .flex_col()
            .gap(px(3.))
            .child(heading("Foreign keys"))
            .children(info.foreign_keys.iter().enumerate().map(|(at, key)| {
                let opening = Tab::Table {
                    id: id.to_owned(),
                    schema: key.schema.clone(),
                    name: key.table.clone(),
                };
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .child(ui::mono(key.columns.clone(), 11.5, cx).text_color(t.text_primary))
                    .child(ui::meta("→", cx))
                    .child(
                        div()
                            .id(("db-foreign-key", at))
                            .cursor_pointer()
                            .text_color(t.accent)
                            .child(ui::mono(key.target.clone(), 11.5, cx))
                            .on_click(listen::<Database, _>(cx, move |database, _, _, _, cx| {
                                database.show(opening.clone());
                                cx.notify();
                            })),
                    )
            }))
    });
    div()
        .id("db-table-page")
        .size_full()
        .overflow_y_scroll()
        .px(SPACE_5)
        .py(SPACE_4)
        .flex()
        .flex_col()
        .gap(SPACE_2H)
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(
                    div()
                        .flex()
                        .child(
                            ui::text(format!("{}.", info.schema), 15., FontWeight::NORMAL)
                                .text_color(t.text_tertiary),
                        )
                        .child(
                            ui::text(info.name.clone(), 15., FontWeight::SEMIBOLD)
                                .text_color(t.text_primary),
                        ),
                )
                .children((info.kind != TableKind::Table).then(|| ui::caps(&info.kind_name, cx)))
                .child(div().flex_1())
                .child(switch),
        )
        .child(heading("Columns"))
        .child(div().flex().flex_col().child(header).children(columns))
        .children(indexes)
        .children(keys)
        .into_any_element()
}

fn history_row(entry: &HistoryEntry, cx: &mut Context<PluginPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let said = match (&entry.error, entry.rows) {
        (Some(error), _) => Some(
            ui::text(error.clone(), 11., FontWeight::NORMAL)
                .text_color(t.danger)
                .truncate(),
        ),
        (None, Some(rows)) => Some(ui::meta(format!("{rows} rows"), cx)),
        (None, None) => None,
    };
    let ago = match crate::ui::time::age(entry.at) {
        now if now == "now" => now,
        age => format!("{age} ago"),
    };
    div()
        .px(SPACE_2H)
        .py(SPACE_2)
        .rounded(RADIUS_2)
        .border_1()
        .border_color(t.border)
        .flex()
        .flex_col()
        .gap(SPACE_1)
        .child(
            div()
                .max_h(px(80.))
                .overflow_hidden()
                .font_family(theme::MONO)
                .text_size(theme::ui_text(TEXT_2XS_H))
                .text_color(t.text_primary)
                .child(entry.sql.clone()),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(ui::meta(ago, cx))
                .child(ui::meta(format!("{} ms", entry.millis), cx))
                .children(said)
                .children(entry.by_agent.then(|| ui::caps("agent", cx))),
        )
        .into_any_element()
}

/// One row of the explorer's tree: its chevron when it opens, its icon and its name; what
/// follows the name is added after.
fn node(
    id: String,
    depth: usize,
    open: Option<bool>,
    current: bool,
    icon: Option<(Icon, Hsla)>,
    label: impl Into<SharedString>,
    cx: &App,
) -> Stateful<Div> {
    let t = Tokens::get(cx).clone();
    let hover = t.surface_hover;
    let chevron = open.map(|open| {
        ui::icon(Icon::Chevron, 9.)
            .when(open, |chevron| {
                chevron.with_transformation(Transformation::rotate(radians(FRAC_PI_2)))
            })
            .text_color(t.text_tertiary)
    });
    div()
        .id(ElementId::Name(id.into()))
        .h(px(24.))
        .pl(px(6. + depth as f32 * 14.))
        .pr(SPACE_1H)
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .rounded(RADIUS_2)
        .cursor_pointer()
        .when(current, |row| row.bg(t.surface_chosen))
        .when(!current, |row| row.hover(move |row| row.bg(hover)))
        .child(div().w(px(10.)).flex_none().children(chevron))
        .children(icon.map(|(icon, tint)| ui::icon(icon, 13.).text_color(tint)))
        .child(
            ui::text(label, 12., FontWeight::NORMAL)
                .text_color(t.text_primary)
                .flex_1()
                .min_w_0()
                .truncate(),
        )
}

fn note(depth: usize, text: SharedString, error: bool, t: &Tokens) -> AnyElement {
    div()
        .pl(px(6. + depth as f32 * 14. + 26.))
        .py(px(3.))
        .text_size(theme::ui_text(TEXT_2XS_H))
        .text_color(if error { t.danger } else { t.text_tertiary })
        .child(text)
        .into_any_element()
}

/// The connection `id` names, or a database on a whole server.
fn connection_of<'a>(connections: &'a [Connection], id: &str) -> Option<&'a Connection> {
    connections
        .iter()
        .find_map(|connection| match connection.id == id {
            true => Some(connection),
            false => connection_of(connection.databases.as_deref().unwrap_or_default(), id),
        })
}

/// A connection's menu; a database on a whole server is edited with the server.
fn connection_menu(
    connection: &Connection,
    nested: bool,
    pane: WeakEntity<PluginPane>,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    let connection = connection.clone();
    move |popup, _, _| {
        let id = connection.id.clone();
        let toggle = match &connection.connected {
            Some(_) => {
                let id = id.clone();
                PopupMenuItem::new("Disconnect")
                    .icon(Icon::Power)
                    .on_click(on(&pane, move |database, host, _, cx| {
                        database.write(host, "disconnect", json!({ "id": id }), cx)
                    }))
            }
            None => {
                let picked = connection.clone();
                PopupMenuItem::new("Connect")
                    .icon(Icon::Plug)
                    .on_click(on(&pane, move |database, host, _, cx| {
                        database.connect(host, &picked, cx)
                    }))
            }
        };
        let (history, properties, fresh, editing) =
            (id.clone(), id.clone(), id.clone(), id.clone());
        popup
            .item(toggle)
            .item(
                PopupMenuItem::new("New console")
                    .icon(Icon::Plus)
                    .disabled(connection.connected.is_none())
                    .on_click(on(&pane, move |database, _, _, _| {
                        database.show(database.new_console(&fresh))
                    })),
            )
            .item(PopupMenuItem::new("History").icon(Icon::Clock).on_click(on(
                &pane,
                move |database, _, _, _| {
                    database.show(Tab::History {
                        id: history.clone(),
                    })
                },
            )))
            .item(
                PopupMenuItem::new("Refresh")
                    .icon(Icon::Refresh)
                    .disabled(connection.connected.is_none())
                    .on_click(on(&pane, |_, host, _, _| host.read_again(None))),
            )
            .separator()
            .item(
                PopupMenuItem::new("Properties")
                    .icon(Icon::InfoCircle)
                    .on_click(on(&pane, move |database, _, _, _| {
                        database.show(Tab::Properties {
                            id: properties.clone(),
                        })
                    })),
            )
            .when(!nested, |popup| {
                popup.item(
                    PopupMenuItem::new("Edit connection…")
                        .icon(Icon::Pencil)
                        .on_click(on(&pane, move |database, host, _, cx| {
                            form::open(database, host, Some(editing.clone()), None, cx)
                        })),
                )
            })
            .item(copy_item(
                "Copy name",
                connection.name.clone(),
                "the name",
                &pane,
            ))
    }
}

/// A schema's menu: a console on its database, the explorer read again, or its name copied.
fn schema_menu(
    id: &str,
    schema: &str,
    pane: WeakEntity<PluginPane>,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    let (id, schema) = (id.to_owned(), schema.to_owned());
    move |popup, _, _| {
        let fresh = id.clone();
        popup
            .item(
                PopupMenuItem::new("New console")
                    .icon(Icon::Plus)
                    .on_click(on(&pane, move |database, _, _, _| {
                        database.show(database.new_console(&fresh))
                    })),
            )
            .item(
                PopupMenuItem::new("Refresh")
                    .icon(Icon::Refresh)
                    .on_click(on(&pane, |_, host, _, _| host.read_again(None))),
            )
            .item(copy_item(
                "Copy name",
                schema.clone(),
                "the schema name",
                &pane,
            ))
    }
}

/// A tab's menu: closing it, or every other tab.
fn tab_menu(
    tab: &Tab,
    alone: bool,
    pane: WeakEntity<PluginPane>,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    let tab = tab.clone();
    move |popup, _, _| {
        let (closed, kept) = (tab.clone(), tab.clone());
        popup
            .item(
                PopupMenuItem::new("Close")
                    .icon(Icon::Close)
                    .on_click(on(&pane, move |database, _, _, _| database.close(&closed))),
            )
            .item(
                PopupMenuItem::new("Close others")
                    .icon(Icon::Close)
                    .disabled(alone)
                    .on_click(on(&pane, move |database, _, _, _| {
                        let others: Vec<Tab> = database
                            .tabs
                            .iter()
                            .filter(|each| **each != kept)
                            .cloned()
                            .collect();
                        for other in &others {
                            database.close(other);
                        }
                    })),
            )
    }
}

fn copy_item(
    label: &'static str,
    text: String,
    what: &'static str,
    pane: &WeakEntity<PluginPane>,
) -> PopupMenuItem {
    PopupMenuItem::new(label)
        .icon(Icon::Copy)
        .on_click(on(pane, move |_, host, _, cx| {
            host.copy(text.clone(), what, cx)
        }))
}

/// A menu item's handler on the Database panel of `pane`.
fn on(
    pane: &WeakEntity<PluginPane>,
    act: impl Fn(&mut Database, &mut Host, &mut Window, &mut Context<PluginPane>) + 'static,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let pane = pane.clone();
    move |_, window, cx| {
        let _ = pane.update(cx, |pane, cx| {
            if let Some((database, host)) = pane.parts::<Database>() {
                act(database, host, window, cx);
            }
            cx.notify();
        });
    }
}
