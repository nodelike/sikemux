// What a database holds, the same for every engine: its schemas, the tables
// and views in each, and one table's columns, indexes and foreign keys.

use serde::{Deserialize, Serialize};

pub use sikemux_plugin_api::database::{Column as ColumnInfo, TableKind};

/// A table's kind from PostgreSQL's `relkind` letter.
pub fn postgres_kind(relkind: &str) -> TableKind {
    match relkind {
        "v" => TableKind::View,
        "m" => TableKind::MaterializedView,
        "f" => TableKind::ForeignTable,
        _ => TableKind::Table,
    }
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Table {
    pub name: String,
    pub kind: TableKind,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Index {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
    pub primary: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ForeignKey {
    pub name: Option<String>,
    pub columns: Vec<String>,
    pub references_schema: Option<String>,
    pub references_table: String,
    pub references_columns: Vec<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TableInfo {
    pub schema: String,
    pub name: String,
    pub kind: TableKind,
    pub columns: Vec<ColumnInfo>,
    pub indexes: Vec<Index>,
    pub foreign_keys: Vec<ForeignKey>,
}

#[derive(Deserialize, Debug)]
pub struct TableRequest {
    pub id: String,
    #[serde(default)]
    pub schema: Option<String>,
    pub table: String,
}

/// A name written so the database reads it as one identifier, whatever it holds.
pub fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_quoted_with_their_own_quotes_doubled() {
        assert_eq!(quote_identifier("users"), "\"users\"");
        assert_eq!(quote_identifier("odd \"name\""), "\"odd \"\"name\"\"\"");
    }

    #[test]
    fn postgres_relation_kinds_are_named() {
        assert_eq!(postgres_kind("r"), TableKind::Table);
        assert_eq!(postgres_kind("p"), TableKind::Table);
        assert_eq!(postgres_kind("v"), TableKind::View);
        assert_eq!(postgres_kind("m"), TableKind::MaterializedView);
        assert_eq!(postgres_kind("f"), TableKind::ForeignTable);
        assert_eq!(
            serde_json::to_value(TableKind::MaterializedView).unwrap(),
            "materialized-view"
        );
    }
}
