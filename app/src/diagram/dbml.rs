//! What a DBML file says: its tables, their columns, how they refer to each other and the
//! groups they are drawn in. Reading is forgiving: what it cannot make sense of it skips.

use std::collections::HashMap;

#[derive(Default)]
pub(super) struct Schema {
    pub tables: Vec<Table>,
    pub refs: Vec<Ref>,
    pub groups: Vec<Group>,
}

pub(super) struct Table {
    /// The name as written, with its schema unless that is `public`.
    pub name: String,
    pub color: Option<u32>,
    pub note: Option<String>,
    pub columns: Vec<Column>,
    /// The line the table starts on, counted from 0.
    pub line: u32,
}

#[derive(Default)]
pub(super) struct Column {
    pub name: String,
    pub kind: String,
    pub pk: bool,
    pub not_null: bool,
    pub unique: bool,
    pub increment: bool,
    pub note: Option<String>,
    pub default: Option<String>,
}

/// One end of a reference: a table and the columns it names there.
#[derive(Clone)]
pub(super) struct End {
    pub table: usize,
    pub columns: Vec<usize>,
    /// Whether many rows can stand at this end.
    pub many: bool,
}

pub(super) struct Ref {
    pub from: End,
    pub to: End,
}

pub(super) struct Group {
    pub name: String,
    pub color: Option<u32>,
    pub tables: Vec<usize>,
}

#[derive(Clone, PartialEq, Debug)]
enum Tok {
    Word(String),
    Text(String),
    Sym(char),
    Line,
}

#[derive(Clone, Debug)]
struct Token {
    tok: Tok,
    start: usize,
    end: usize,
    line: u32,
}

fn lex(source: &str) -> Vec<Token> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut at = 0;
    let mut line = 0u32;
    let lines_in = |from: usize, to: usize| source[from..to].matches('\n').count() as u32;
    while at < bytes.len() {
        let c = bytes[at];
        let start = at;
        match c {
            b'\n' => {
                tokens.push(Token {
                    tok: Tok::Line,
                    start,
                    end: at + 1,
                    line,
                });
                line += 1;
                at += 1;
            }
            b' ' | b'\t' | b'\r' => at += 1,
            b'/' if bytes.get(at + 1) == Some(&b'/') => {
                while at < bytes.len() && bytes[at] != b'\n' {
                    at += 1;
                }
            }
            b'/' if bytes.get(at + 1) == Some(&b'*') => {
                let end = source[at + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |end| at + 2 + end + 2);
                line += lines_in(at, end);
                at = end;
            }
            b'\'' | b'"' | b'`' => {
                let triple = c == b'\'' && source[at..].starts_with("'''");
                let (open, close) = if triple {
                    (3, "'''")
                } else {
                    (1, &source[at..at + 1])
                };
                let mut text = String::new();
                let mut cursor = at + open;
                let mut chars = source[cursor..].char_indices();
                let mut end = bytes.len();
                while let Some((offset, ch)) = chars.next() {
                    if ch == '\\' {
                        if let Some((_, next)) = chars.next() {
                            text.push(next);
                        }
                        continue;
                    }
                    if source[cursor + offset..].starts_with(close) {
                        end = cursor + offset + close.len();
                        break;
                    }
                    text.push(ch);
                }
                cursor = end;
                let first = line;
                line += lines_in(at, cursor);
                tokens.push(Token {
                    tok: Tok::Text(if triple { dedent(&text) } else { text }),
                    start,
                    end: cursor,
                    line: first,
                });
                at = cursor;
            }
            b'#' if bytes.get(at + 1).is_some_and(u8::is_ascii_hexdigit) => {
                at += 1;
                while at < bytes.len() && bytes[at].is_ascii_alphanumeric() {
                    at += 1;
                }
                tokens.push(Token {
                    tok: Tok::Word(source[start..at].to_string()),
                    start,
                    end: at,
                    line,
                });
            }
            c if c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80 => {
                while at < bytes.len()
                    && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_' || bytes[at] >= 0x80)
                {
                    at += 1;
                }
                tokens.push(Token {
                    tok: Tok::Word(source[start..at].to_string()),
                    start,
                    end: at,
                    line,
                });
            }
            _ => {
                tokens.push(Token {
                    tok: Tok::Sym(c as char),
                    start,
                    end: at + 1,
                    line,
                });
                at += 1;
            }
        }
    }
    tokens
}

/// A `'''` note's text without the indent its lines share.
fn dedent(text: &str) -> String {
    let text = text.trim_matches('\n');
    let indent = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);
    text.lines()
        .map(|line| line.get(indent..).unwrap_or(line.trim_start()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A reference as written, before its names are looked up.
struct Written {
    from: (Vec<String>, Vec<String>),
    op: &'static str,
    to: (Vec<String>, Vec<String>),
}

#[derive(Default)]
struct Settings {
    pk: bool,
    not_null: bool,
    unique: bool,
    increment: bool,
    note: Option<String>,
    default: Option<String>,
    color: Option<u32>,
    refs: Vec<(&'static str, Vec<String>)>,
}

struct Parser<'a> {
    source: &'a str,
    tokens: Vec<Token>,
    at: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.at).map(|token| &token.tok)
    }

    fn peek_at(&self, ahead: usize) -> Option<&Tok> {
        self.tokens.get(self.at + ahead).map(|token| &token.tok)
    }

    fn bump(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.at).cloned();
        self.at += 1;
        token
    }

    fn is_sym(&self, c: char) -> bool {
        self.peek() == Some(&Tok::Sym(c))
    }

    fn eat_sym(&mut self, c: char) -> bool {
        let found = self.is_sym(c);
        if found {
            self.at += 1;
        }
        found
    }

    fn skip_lines(&mut self) {
        while self.peek() == Some(&Tok::Line) {
            self.at += 1;
        }
    }

    fn skip_line(&mut self) {
        while !matches!(self.peek(), None | Some(Tok::Line)) {
            if self.is_sym('{') {
                self.skip_block();
                continue;
            }
            self.at += 1;
        }
    }

    /// Steps over everything up to and past the `}` closing the next `{`.
    fn skip_block(&mut self) {
        while !matches!(self.peek(), None | Some(Tok::Sym('{'))) {
            self.at += 1;
        }
        let mut depth = 0;
        while let Some(token) = self.bump() {
            match token.tok {
                Tok::Sym('{') => depth += 1,
                Tok::Sym('}') => {
                    depth -= 1;
                    if depth == 0 {
                        return;
                    }
                }
                _ => {}
            }
        }
    }

    fn word(&mut self) -> Option<String> {
        match self.peek()? {
            Tok::Word(word) | Tok::Text(word) => {
                let word = word.clone();
                self.at += 1;
                Some(word)
            }
            _ => None,
        }
    }

    /// A dotted name, `schema.table.column`, as its parts.
    fn name(&mut self) -> Vec<String> {
        let mut parts = Vec::new();
        let Some(first) = self.word() else {
            return parts;
        };
        parts.push(first);
        while self.is_sym('.') && matches!(self.peek_at(1), Some(Tok::Word(_) | Tok::Text(_))) {
            self.at += 1;
            parts.extend(self.word());
        }
        parts
    }

    /// A reference's end: a dotted name, perhaps ending in `(a, b)` for several columns.
    fn end(&mut self) -> (Vec<String>, Vec<String>) {
        let mut parts = self.name();
        if self.is_sym('.') && self.peek_at(1) == Some(&Tok::Sym('(')) {
            self.at += 2;
            let mut columns = Vec::new();
            while let Some(tok) = self.peek().cloned() {
                self.at += 1;
                match tok {
                    Tok::Word(word) | Tok::Text(word) => columns.push(word),
                    Tok::Sym(')') => break,
                    _ => {}
                }
            }
            return (parts, columns);
        }
        let column = parts.pop().into_iter().collect();
        (parts, column)
    }

    fn op(&mut self) -> Option<&'static str> {
        let op = match (self.peek(), self.peek_at(1)) {
            (Some(Tok::Sym('<')), Some(Tok::Sym('>'))) => {
                self.at += 1;
                "<>"
            }
            (Some(Tok::Sym('<')), _) => "<",
            (Some(Tok::Sym('>')), _) => ">",
            (Some(Tok::Sym('-')), _) => "-",
            _ => return None,
        };
        self.at += 1;
        Some(op)
    }

    /// The `[...]` after a column, table or reference, read to its closing bracket.
    fn settings(&mut self) -> Settings {
        let mut settings = Settings::default();
        if !self.eat_sym('[') {
            return settings;
        }
        loop {
            while self.peek() == Some(&Tok::Line) || self.is_sym(',') {
                self.at += 1;
            }
            let Some(head) = self.peek().cloned() else {
                break;
            };
            if head == Tok::Sym(']') {
                self.at += 1;
                break;
            }
            let item_start = self.at;
            let key = match head {
                Tok::Word(word) => word.to_ascii_lowercase(),
                _ => String::new(),
            };
            self.at += 1;
            let valued = self.eat_sym(':');
            match key.as_str() {
                "pk" | "primary" => settings.pk = true,
                "not" => settings.not_null = true,
                "unique" => settings.unique = true,
                "increment" => settings.increment = true,
                "note" if valued => {
                    if let Some(Tok::Text(text)) = self.peek().cloned() {
                        settings.note = Some(text);
                    }
                }
                "headercolor" | "color" if valued => {
                    if let Some(Tok::Word(word)) = self.peek().cloned() {
                        settings.color = hex(&word);
                    }
                }
                "ref" if valued => {
                    if let Some(op) = self.op() {
                        let (mut table, columns) = self.end();
                        table.extend(columns);
                        settings.refs.push((op, table));
                    }
                }
                "default" if valued => {
                    let from = self.tokens.get(self.at).map(|token| token.start);
                    let mut to = from;
                    let mut text = None;
                    while !matches!(self.peek(), None | Some(Tok::Sym(',' | ']'))) {
                        let token = self.bump().unwrap();
                        if let Tok::Text(value) = &token.tok {
                            text = Some(value.clone());
                        }
                        to = Some(token.end);
                    }
                    settings.default = match (text, from, to) {
                        (Some(text), _, _) => Some(text),
                        (None, Some(from), Some(to)) => Some(self.source[from..to].to_string()),
                        _ => None,
                    };
                }
                _ => {}
            }
            let mut depth = 0;
            while let Some(tok) = self.peek() {
                match tok {
                    Tok::Sym('(') => depth += 1,
                    Tok::Sym(')') => depth -= 1,
                    Tok::Sym(',') | Tok::Sym(']') if depth <= 0 => break,
                    _ => {}
                }
                self.at += 1;
            }
            if self.at == item_start {
                self.at += 1;
            }
        }
        settings
    }
}

fn hex(word: &str) -> Option<u32> {
    let digits = word.strip_prefix('#')?;
    let digits = match digits.len() {
        3 => digits.chars().flat_map(|c| [c, c]).collect(),
        6 => digits.to_string(),
        _ => return None,
    };
    u32::from_str_radix(&digits, 16).ok()
}

struct Draft {
    table: Table,
    aliases: Vec<String>,
    inline: Vec<(usize, &'static str, Vec<String>)>,
    pk_sets: Vec<Vec<String>>,
}

pub(super) fn parse(source: &str) -> Schema {
    let mut parser = Parser {
        source,
        tokens: lex(source),
        at: 0,
    };
    let mut drafts: Vec<Draft> = Vec::new();
    let mut written: Vec<Written> = Vec::new();
    let mut groups: Vec<(String, Option<u32>, Vec<Vec<String>>)> = Vec::new();
    while let Some(tok) = parser.peek().cloned() {
        let Tok::Word(word) = tok else {
            parser.at += 1;
            continue;
        };
        match word.to_ascii_lowercase().as_str() {
            "table" => {
                let line = parser.tokens[parser.at].line;
                parser.at += 1;
                drafts.push(table(&mut parser, line));
            }
            "ref" => {
                parser.at += 1;
                refs(&mut parser, &mut written);
            }
            "tablegroup" => {
                parser.at += 1;
                let name = parser.name().join(".");
                let color = parser.settings().color;
                let mut members = Vec::new();
                if parser.eat_sym('{') {
                    loop {
                        parser.skip_lines();
                        match parser.peek() {
                            None => break,
                            Some(Tok::Sym('}')) => {
                                parser.at += 1;
                                break;
                            }
                            Some(Tok::Word(word)) if word.eq_ignore_ascii_case("note") => {
                                parser.skip_line()
                            }
                            _ => {
                                let member = parser.name();
                                if member.is_empty() {
                                    parser.at += 1;
                                } else {
                                    members.push(member);
                                }
                                parser.skip_line();
                            }
                        }
                    }
                }
                groups.push((name, color, members));
            }
            "enum" | "project" | "tablepartial" | "records" => parser.skip_block(),
            "note" => {
                parser.at += 1;
                match parser.peek() {
                    Some(Tok::Sym(':')) => parser.skip_line(),
                    _ => parser.skip_block(),
                }
            }
            _ => parser.at += 1,
        }
    }
    resolve(drafts, written, groups)
}

fn table(parser: &mut Parser, line: u32) -> Draft {
    let parts = parser.name();
    let mut aliases = vec![parts.join(".")];
    if parts.len() == 2 && parts[0].eq_ignore_ascii_case("public") {
        aliases.push(parts[1].clone());
    }
    let name = match parts.as_slice() {
        [schema, name] if schema.eq_ignore_ascii_case("public") => name.clone(),
        _ => parts.join("."),
    };
    if matches!(parser.peek(), Some(Tok::Word(word)) if word.eq_ignore_ascii_case("as")) {
        parser.at += 1;
        aliases.extend(parser.word());
    }
    let settings = parser.settings();
    let mut draft = Draft {
        table: Table {
            name,
            color: settings.color,
            note: settings.note,
            columns: Vec::new(),
            line,
        },
        aliases,
        inline: Vec::new(),
        pk_sets: Vec::new(),
    };
    if !parser.eat_sym('{') {
        return draft;
    }
    loop {
        parser.skip_lines();
        let Some(tok) = parser.peek().cloned() else {
            break;
        };
        match tok {
            Tok::Sym('}') => {
                parser.at += 1;
                break;
            }
            Tok::Sym('~') => parser.skip_line(),
            Tok::Word(word)
                if word.eq_ignore_ascii_case("indexes")
                    && parser.peek_at(1) == Some(&Tok::Sym('{')) =>
            {
                parser.at += 2;
                indexes(parser, &mut draft);
            }
            Tok::Word(word)
                if word.eq_ignore_ascii_case("note")
                    && matches!(parser.peek_at(1), Some(Tok::Sym(':' | '{'))) =>
            {
                parser.at += 1;
                let braced = parser.eat_sym('{');
                parser.eat_sym(':');
                parser.skip_lines();
                if let Some(Tok::Text(text)) = parser.peek().cloned() {
                    draft.table.note = Some(text);
                    parser.at += 1;
                }
                if braced {
                    parser.skip_lines();
                    parser.eat_sym('}');
                }
                parser.skip_line();
            }
            Tok::Word(_) | Tok::Text(_) => column(parser, &mut draft),
            _ => parser.skip_line(),
        }
    }
    draft
}

fn column(parser: &mut Parser, draft: &mut Draft) {
    let name = parser.word().unwrap_or_default();
    let mut kind = String::new();
    let mut from = None;
    let mut to = 0;
    loop {
        match parser.peek() {
            None | Some(Tok::Line) | Some(Tok::Sym('}')) => break,
            Some(Tok::Sym('[')) if parser.peek_at(1) != Some(&Tok::Sym(']')) => break,
            Some(Tok::Text(text)) if from.is_none() => {
                kind = text.clone();
                parser.at += 1;
                to = usize::MAX;
            }
            _ => {
                let token = parser.bump().unwrap();
                from.get_or_insert(token.start);
                to = token.end;
            }
        }
    }
    if let Some(from) = from.filter(|_| to != usize::MAX) {
        kind = parser.source[from..to]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
    }
    let settings = parser.settings();
    let index = draft.table.columns.len();
    for (op, target) in settings.refs {
        draft.inline.push((index, op, target));
    }
    draft.table.columns.push(Column {
        name,
        kind,
        pk: settings.pk,
        not_null: settings.not_null,
        unique: settings.unique,
        increment: settings.increment,
        note: settings.note,
        default: settings.default,
    });
    parser.skip_line();
}

fn indexes(parser: &mut Parser, draft: &mut Draft) {
    loop {
        parser.skip_lines();
        let mut columns = Vec::new();
        match parser.peek() {
            None => return,
            Some(Tok::Sym('}')) => {
                parser.at += 1;
                return;
            }
            Some(Tok::Sym('(')) => {
                parser.at += 1;
                while let Some(tok) = parser.bump() {
                    match tok.tok {
                        Tok::Word(word) | Tok::Text(word) => columns.push(word),
                        Tok::Sym(')') | Tok::Line => break,
                        _ => {}
                    }
                }
            }
            Some(Tok::Word(_) | Tok::Text(_)) => columns.extend(parser.word()),
            _ => {}
        }
        while !matches!(
            parser.peek(),
            None | Some(Tok::Line | Tok::Sym('[') | Tok::Sym('}'))
        ) {
            parser.at += 1;
        }
        let settings = parser.settings();
        if settings.pk {
            draft.pk_sets.push(columns);
        } else if settings.unique && columns.len() == 1 {
            if let Some(column) = (draft.table.columns.iter_mut()).find(|c| c.name == columns[0]) {
                column.unique = true;
            }
        }
        parser.skip_line();
    }
}

fn refs(parser: &mut Parser, written: &mut Vec<Written>) {
    if !parser.is_sym(':') && !parser.is_sym('{') {
        parser.name();
    }
    let one = |parser: &mut Parser| -> Option<Written> {
        let from = parser.end();
        let op = parser.op()?;
        let to = parser.end();
        parser.settings();
        Some(Written { from, op, to })
    };
    if parser.eat_sym(':') {
        written.extend(one(parser));
        parser.skip_line();
        return;
    }
    if !parser.eat_sym('{') {
        return;
    }
    loop {
        parser.skip_lines();
        match parser.peek() {
            None => return,
            Some(Tok::Sym('}')) => {
                parser.at += 1;
                return;
            }
            _ => {
                let before = parser.at;
                written.extend(one(parser));
                if parser.at == before {
                    parser.at += 1;
                }
                parser.skip_line();
            }
        }
    }
}

fn resolve(
    drafts: Vec<Draft>,
    written: Vec<Written>,
    groups: Vec<(String, Option<u32>, Vec<Vec<String>>)>,
) -> Schema {
    let mut by_name: HashMap<String, usize> = HashMap::new();
    for (index, draft) in drafts.iter().enumerate() {
        for alias in &draft.aliases {
            by_name.entry(alias.to_lowercase()).or_insert(index);
        }
    }
    let find_table = |parts: &[String]| -> Option<usize> {
        let name = parts.join(".").to_lowercase();
        by_name
            .get(&name)
            .or_else(|| by_name.get(&format!("public.{name}")))
            .copied()
    };
    let mut written = written;
    let mut tables = Vec::with_capacity(drafts.len());
    for draft in drafts {
        let Draft {
            mut table,
            inline,
            pk_sets,
            aliases,
        } = draft;
        for set in pk_sets {
            for column in &mut table.columns {
                if set.iter().any(|name| name == &column.name) {
                    column.pk = true;
                }
            }
        }
        for (column, op, mut target) in inline {
            let to_column = target.pop().unwrap_or_default();
            written.push(Written {
                from: (
                    vec![aliases[0].clone()],
                    vec![table.columns[column].name.clone()],
                ),
                op,
                to: (target, vec![to_column]),
            });
        }
        tables.push(table);
    }
    let end = |tables: &[Table], (parts, columns): &(Vec<String>, Vec<String>), many: bool| {
        let table = find_table(parts)?;
        let columns = columns
            .iter()
            .filter_map(|name| tables[table].columns.iter().position(|c| &c.name == name))
            .collect();
        Some(End {
            table,
            columns,
            many,
        })
    };
    let refs = written
        .iter()
        .filter_map(|written| {
            let (from_many, to_many) = match written.op {
                ">" => (true, false),
                "<" => (false, true),
                "<>" => (true, true),
                _ => (false, false),
            };
            Some(Ref {
                from: end(&tables, &written.from, from_many)?,
                to: end(&tables, &written.to, to_many)?,
            })
        })
        .collect();
    let groups = groups
        .into_iter()
        .map(|(name, color, members)| Group {
            name,
            color,
            tables: members
                .iter()
                .filter_map(|parts| find_table(parts))
                .collect(),
        })
        .collect();
    Schema {
        tables,
        refs,
        groups,
    }
}
