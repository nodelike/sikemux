// What the person's queries show: each result's cells as text, a line about it, the text an
// agent is handed of it, and the copies a spreadsheet takes.

use serde::Deserialize;
use serde_json::Value;
use sikemux_plugin_api::database::{CopyFormat, QueryOutcome, ResultColumn, ResultSet};

use crate::values;

/// Rows of a result an agent is handed, so a big one does not flood its context.
const AGENT_ROWS: usize = 50;

/// Where the SQL ran, as an agent is told it.
pub struct Ran<'a> {
    pub database: &'a str,
    pub engine: &'a str,
    pub sql: &'a str,
}

pub fn shown(outcome: values::QueryOutcome, ran: &Ran) -> QueryOutcome {
    let results: Vec<ResultSet> = outcome
        .results
        .into_iter()
        .map(|result| result_set(result, ran))
        .collect();
    let main = results
        .iter()
        .rposition(|result| !result.columns.is_empty())
        .unwrap_or(results.len().saturating_sub(1));
    QueryOutcome {
        results,
        main,
        millis: outcome.millis,
    }
}

fn result_set(result: values::ResultSet, ran: &Ran) -> ResultSet {
    let mut shown = ResultSet {
        columns: result
            .columns
            .into_iter()
            .map(|column| ResultColumn {
                name: column.name,
                type_name: column.type_name,
                numeric: column.numeric,
            })
            .collect(),
        rows: result
            .rows
            .iter()
            .map(|row| row.iter().map(cell).collect())
            .collect(),
        truncated: result.truncated,
        affected: result.affected,
        summary: String::new(),
        for_agent: String::new(),
    };
    shown.summary = summary(&shown);
    shown.for_agent = format!(
        "Results of a query on the {} database \"{}\":\n\n{}\n\n{}",
        ran.engine,
        ran.database,
        fenced(ran.sql),
        markdown(&shown, AGENT_ROWS)
    );
    shown
}

fn cell(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        other => Some(other.to_string()),
    }
}

/// `count` with its noun, the count grouped in thousands: "1 row", "1,204 rows".
fn counted(count: usize, one: &str, many: &str) -> String {
    let digits = count.to_string();
    let mut grouped = String::new();
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    format!("{grouped} {}", if count == 1 { one } else { many })
}

fn summary(result: &ResultSet) -> String {
    if let (Some(affected), true) = (result.affected, result.columns.is_empty()) {
        let affected = usize::try_from(affected).unwrap_or(usize::MAX);
        return format!("{} changed", counted(affected, "row", "rows"));
    }
    let rows = counted(result.rows.len(), "row", "rows");
    match result.truncated {
        true => format!("First {rows}; more were left out"),
        false => rows,
    }
}

/// The text with every run of `these` characters as one space.
fn squashed(text: &str, these: &[char]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_run = false;
    for character in text.chars() {
        match these.contains(&character) {
            true if in_run => {}
            true => {
                out.push(' ');
                in_run = true;
            }
            false => {
                out.push(character);
                in_run = false;
            }
        }
    }
    out
}

/// A markdown table of the first `rows` rows, numbers to the right, saying what was left out.
fn markdown(result: &ResultSet, rows: usize) -> String {
    if result.columns.is_empty() {
        return result.summary.clone();
    }
    let field = |text: &str| squashed(&text.replace('|', "\\|"), &['\r', '\n']);
    let line = |cells: Vec<String>| format!("| {} |", cells.join(" | "));
    let mut lines = vec![
        line(
            result
                .columns
                .iter()
                .map(|column| field(&column.name))
                .collect(),
        ),
        line(
            result
                .columns
                .iter()
                .map(|column| if column.numeric { "---:" } else { "---" }.to_owned())
                .collect(),
        ),
    ];
    lines.extend(result.rows.iter().take(rows).map(|row| {
        line(
            row.iter()
                .map(|cell| field(cell.as_deref().unwrap_or("NULL")))
                .collect(),
        )
    }));
    let left = result.rows.len().saturating_sub(rows);
    let mut table = lines.join("\n");
    if left > 0 || result.truncated {
        table.push_str(&format!("\n\n{}", result.summary));
        if left > 0 {
            table.push_str(&format!(
                "; {} not shown",
                counted(left, "more row", "more rows")
            ));
        }
    }
    table
}

/// The SQL in a fenced block no run of backticks in it can close.
fn fenced(sql: &str) -> String {
    let longest = sql
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat((longest + 1).max(3));
    format!("{fence}sql\n{}\n{fence}", sql.trim_end_matches('\n'))
}

/// A result as the client has it, to copy.
#[derive(Deserialize)]
pub struct CopyAsk {
    pub result: Copied,
    pub format: CopyFormat,
}

#[derive(Deserialize)]
pub struct Copied {
    pub columns: Vec<ResultColumn>,
    pub rows: Vec<Vec<Option<String>>>,
}

/// The result with a header row, tab-separated as spreadsheets paste it, or as CSV with
/// fields quoted where they need it. NULL is an empty field either way.
pub fn copied(ask: CopyAsk) -> String {
    let names = ask
        .result
        .columns
        .iter()
        .map(|column| Some(column.name.clone()));
    let header: Vec<Option<String>> = names.collect();
    let rows = std::iter::once(&header).chain(ask.result.rows.iter());
    match ask.format {
        CopyFormat::Tsv => rows
            .map(|row| {
                row.iter()
                    .map(|cell| squashed(cell.as_deref().unwrap_or(""), &['\t', '\n', '\r']))
                    .collect::<Vec<_>>()
                    .join("\t")
            })
            .collect::<Vec<_>>()
            .join("\n"),
        CopyFormat::Csv => rows
            .map(|row| {
                row.iter()
                    .map(|cell| match cell.as_deref() {
                        None => String::new(),
                        Some(text) if text.contains(['"', ',', '\r', '\n']) => {
                            format!("\"{}\"", text.replace('"', "\"\""))
                        }
                        Some(text) => text.to_owned(),
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .collect::<Vec<_>>()
            .join("\r\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(name: &str, type_name: &str, numeric: bool) -> values::Column {
        values::Column {
            name: name.into(),
            type_name: type_name.into(),
            numeric,
        }
    }

    fn customers() -> values::ResultSet {
        values::ResultSet {
            columns: vec![
                column("id", "int4", true),
                column("name", "text", false),
                column("note", "text", false),
            ],
            rows: vec![
                vec![Value::from(1), "Ada".into(), "likes | pipes".into()],
                vec![Value::from(2), "Linus".into(), Value::Null],
            ],
            truncated: false,
            affected: None,
        }
    }

    fn changed(affected: u64) -> values::ResultSet {
        values::ResultSet {
            affected: Some(affected),
            ..values::ResultSet::default()
        }
    }

    const RAN: Ran = Ran {
        database: "Shop",
        engine: "PostgreSQL",
        sql: "select id, name, note from customers",
    };

    fn outcome(results: Vec<values::ResultSet>) -> QueryOutcome {
        shown(values::QueryOutcome { results, millis: 1 }, &RAN)
    }

    #[test]
    fn shows_each_kind_of_cell() {
        assert_eq!(cell(&Value::Null), None);
        assert_eq!(cell(&Value::Bool(true)).as_deref(), Some("true"));
        assert_eq!(cell(&serde_json::json!(42.5)).as_deref(), Some("42.5"));
        assert_eq!(
            cell(&"9223372036854775807".into()).as_deref(),
            Some("9223372036854775807")
        );
    }

    #[test]
    fn sums_up_rows_returned_rows_changed_and_rows_left_out() {
        let truncated = values::ResultSet {
            truncated: true,
            ..customers()
        };
        let summaries: Vec<String> = outcome(vec![customers(), changed(3), changed(1), truncated])
            .results
            .into_iter()
            .map(|result| result.summary)
            .collect();
        assert_eq!(
            summaries,
            [
                "2 rows",
                "3 rows changed",
                "1 row changed",
                "First 2 rows; more were left out"
            ]
        );
        assert_eq!(counted(12_345, "row", "rows"), "12,345 rows");
    }

    #[test]
    fn shows_the_last_result_that_returned_rows() {
        assert_eq!(outcome(vec![customers(), changed(3)]).main, 0);
        assert_eq!(outcome(vec![changed(3), customers()]).main, 1);
        assert_eq!(outcome(vec![changed(3), changed(3)]).main, 1);
        assert_eq!(outcome(Vec::new()).main, 0);
    }

    #[test]
    fn hands_an_agent_the_database_the_sql_and_the_table() {
        let result = &outcome(vec![customers()]).results[0];
        assert!(result
            .for_agent
            .starts_with("Results of a query on the PostgreSQL database \"Shop\":"));
        assert!(result
            .for_agent
            .contains("```sql\nselect id, name, note from customers\n```"));
        assert_eq!(
            markdown(result, AGENT_ROWS),
            [
                "| id | name | note |",
                "| ---: | --- | --- |",
                "| 1 | Ada | likes \\| pipes |",
                "| 2 | Linus | NULL |"
            ]
            .join("\n")
        );
        assert!(markdown(result, 1).contains("2 rows; 1 more row not shown"));
        assert_eq!(
            markdown(&outcome(vec![changed(3)]).results[0], AGENT_ROWS),
            "3 rows changed"
        );
    }

    #[test]
    fn copies_as_tab_separated_text_or_csv_with_null_left_empty() {
        let result = &outcome(vec![customers()]).results[0];
        let ask = |format| CopyAsk {
            result: Copied {
                columns: result.columns.clone(),
                rows: result.rows.clone(),
            },
            format,
        };
        assert_eq!(
            copied(ask(CopyFormat::Tsv)),
            "id\tname\tnote\n1\tAda\tlikes | pipes\n2\tLinus\t"
        );
        let tricky = CopyAsk {
            result: Copied {
                columns: result.columns.clone(),
                rows: vec![
                    vec![
                        Some("1".into()),
                        Some("say \"hi\", ok".into()),
                        Some("two\nlines".into()),
                    ],
                    vec![Some("2".into()), Some("plain".into()), None],
                ],
            },
            format: CopyFormat::Csv,
        };
        assert_eq!(
            copied(tricky),
            "id,name,note\r\n1,\"say \"\"hi\"\", ok\",\"two\nlines\"\r\n2,plain,"
        );
    }
}
