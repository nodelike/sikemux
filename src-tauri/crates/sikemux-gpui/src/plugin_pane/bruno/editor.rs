//! A request being edited: a field for each thing the person can type, read back into the
//! request whenever one changes. What the fields do not cover is kept from the request as it
//! was opened, so saving gives it back unchanged.

use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::{App, AppContext, Context, Entity, Subscription, Window};
use sikemux_wire::bruno::{AuthMode, BodyMode, HttpMethod, KeyPlacement, KeyVal, Request};

use super::{Bruno, PluginPane};

/// The rows a table of names and values edits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Table {
    Query,
    Path,
    Headers,
    Form,
    RequestVars,
    ResponseVars,
    Assertions,
}

impl Table {
    pub const ALL: [Table; 7] = [
        Table::Query,
        Table::Path,
        Table::Headers,
        Table::Form,
        Table::RequestVars,
        Table::ResponseVars,
        Table::Assertions,
    ];

    fn rows(self, request: &Request) -> &Vec<KeyVal> {
        match self {
            Table::Query => &request.params.query,
            Table::Path => &request.params.path,
            Table::Headers => &request.headers,
            Table::Form => &request.body.form,
            Table::RequestVars => &request.vars.req,
            Table::ResponseVars => &request.vars.res,
            Table::Assertions => &request.assertions,
        }
    }

    fn rows_mut(self, request: &mut Request) -> &mut Vec<KeyVal> {
        match self {
            Table::Query => &mut request.params.query,
            Table::Path => &mut request.params.path,
            Table::Headers => &mut request.headers,
            Table::Form => &mut request.body.form,
            Table::RequestVars => &mut request.vars.req,
            Table::ResponseVars => &mut request.vars.res,
            Table::Assertions => &mut request.assertions,
        }
    }

    /// What its two columns hold, as their empty fields say.
    pub fn hints(self) -> (&'static str, &'static str) {
        match self {
            Table::Query | Table::Path => ("name", "value"),
            Table::Headers => ("header", "value"),
            Table::Form => ("field", "value, or @file(/path)"),
            Table::RequestVars => ("variable", "value"),
            Table::ResponseVars => ("variable", "expression, such as res.body.id"),
            Table::Assertions => ("expression, such as res.status", "eq 200"),
        }
    }
}

pub struct Row {
    pub enabled: bool,
    pub name: Entity<InputState>,
    pub value: Entity<InputState>,
}

/// The text a multi-line field edits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Text {
    Body,
    PreScript,
    PostScript,
    Docs,
}

pub struct Editor {
    /// The file the request is in.
    pub path: String,
    /// The request as opened, which supplies whatever the fields do not cover.
    base: Request,
    pub method: HttpMethod,
    pub body_mode: BodyMode,
    pub auth_mode: AuthMode,
    pub placement: KeyPlacement,
    pub url: Entity<InputState>,
    pub tables: Vec<(Table, Vec<Row>)>,
    pub texts: Vec<(Text, Entity<TextareaState>)>,
    pub token: Entity<InputState>,
    pub username: Entity<InputState>,
    pub password: Entity<InputState>,
    pub key: Entity<InputState>,
    pub key_value: Entity<InputState>,
    _typing: Vec<Subscription>,
}

fn watch<T: gpui_kit::EventEmitter<InputEvent> + 'static>(
    entity: &Entity<T>,
    path: &str,
    cx: &mut Context<PluginPane>,
) -> Subscription {
    let path = path.to_owned();
    cx.subscribe(entity, move |pane, _, event: &InputEvent, cx| {
        if !matches!(event, InputEvent::Change) {
            return;
        }
        if let Some((bruno, _)) = pane.parts::<Bruno>() {
            bruno.edited(&path, cx);
        }
        cx.notify();
    })
}

impl Editor {
    pub fn new(path: &str, request: &Request, window: &mut Window, cx: &mut Context<PluginPane>) -> Self {
        let mut typing = Vec::new();
        let mut line = |value: &str, hint: &str, window: &mut Window, cx: &mut Context<PluginPane>| {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(hint.to_owned())
                    .default_value(value.to_owned())
            });
            typing.push(watch(&input, path, cx));
            input
        };
        let url = line(&request.url, "https://api.example.com/{{path}}", window, cx);
        let token = line(&request.auth.bearer.token, "token", window, cx);
        let username = line(&request.auth.basic.username, "user name", window, cx);
        let password = line(&request.auth.basic.password, "password", window, cx);
        let key = line(&request.auth.apikey.key, "key", window, cx);
        let key_value = line(&request.auth.apikey.value, "value", window, cx);
        let mut tables = Vec::new();
        for table in Table::ALL {
            let (name_hint, value_hint) = table.hints();
            let rows = table
                .rows(request)
                .iter()
                .map(|row| Row {
                    enabled: row.enabled,
                    name: line(&row.name, name_hint, window, cx),
                    value: line(&row.value, value_hint, window, cx),
                })
                .collect();
            tables.push((table, rows));
        }
        let mut texts = Vec::new();
        for (text, value, hint) in [
            (Text::Body, &request.body.text, "Body"),
            (Text::PreScript, &request.scripts.pre, "Runs before the request goes, as bru, req and console allow"),
            (Text::PostScript, &request.scripts.post, "Runs after the response comes, with res, test and expect"),
            (Text::Docs, &request.docs, "Notes about this request"),
        ] {
            let area = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(8, 30)
                    .placeholder(hint)
                    .default_value(value.clone())
            });
            typing.push(watch(&area, path, cx));
            texts.push((text, area));
        }
        Self {
            path: path.to_owned(),
            base: request.clone(),
            method: request.method,
            body_mode: request.body.mode,
            auth_mode: request.auth.mode,
            placement: request.auth.apikey.placement,
            url,
            tables,
            texts,
            token,
            username,
            password,
            key,
            key_value,
            _typing: typing,
        }
    }

    /// A new empty row at the end of `table`.
    pub fn add_row(&mut self, table: Table, window: &mut Window, cx: &mut Context<PluginPane>) {
        let (name_hint, value_hint) = table.hints();
        let mut made = Vec::new();
        for hint in [name_hint, value_hint] {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(hint));
            self._typing.push(watch(&input, &self.path, cx));
            made.push(input);
        }
        let (Some(value), Some(name)) = (made.pop(), made.pop()) else {
            return;
        };
        if let Some((_, rows)) = self.tables.iter_mut().find(|(each, _)| *each == table) {
            rows.push(Row {
                enabled: true,
                name,
                value,
            });
        }
    }

    pub fn rows(&self, table: Table) -> &[Row] {
        self.tables
            .iter()
            .find(|(each, _)| *each == table)
            .map_or(&[], |(_, rows)| rows.as_slice())
    }

    pub fn rows_mut(&mut self, table: Table) -> Option<&mut Vec<Row>> {
        self.tables
            .iter_mut()
            .find(|(each, _)| *each == table)
            .map(|(_, rows)| rows)
    }

    pub fn text(&self, text: Text) -> Option<&Entity<TextareaState>> {
        self.texts.iter().find(|(each, _)| *each == text).map(|(_, area)| area)
    }

    /// The request as the fields now say it.
    pub fn request(&self, cx: &App) -> Request {
        let read = |input: &Entity<InputState>| input.read(cx).value().to_string();
        let read_area = |text: Text| {
            self.text(text)
                .map(|area| area.read(cx).value().to_string())
                .unwrap_or_default()
        };
        let mut request = self.base.clone();
        request.method = self.method;
        request.url = read(&self.url);
        request.body.mode = self.body_mode;
        request.body.text = read_area(Text::Body);
        request.auth.mode = self.auth_mode;
        request.auth.bearer.token = read(&self.token);
        request.auth.basic.username = read(&self.username);
        request.auth.basic.password = read(&self.password);
        request.auth.apikey.key = read(&self.key);
        request.auth.apikey.value = read(&self.key_value);
        request.auth.apikey.placement = self.placement;
        request.scripts.pre = read_area(Text::PreScript);
        request.scripts.post = read_area(Text::PostScript);
        request.docs = read_area(Text::Docs);
        for (table, rows) in &self.tables {
            *table.rows_mut(&mut request) = rows
                .iter()
                .map(|row| KeyVal {
                    name: read(&row.name),
                    value: read(&row.value),
                    enabled: row.enabled,
                })
                .filter(|row| !row.name.is_empty() || !row.value.is_empty())
                .collect();
        }
        request
    }
}
