// BigQuery: datasets, tables with their schemas, a free preview of a table's
// rows, and queries, each estimated with a dry run first and capped in how
// much they may bill, so nothing runs up a surprise.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::Target;
use crate::client::{self, at, segment, text};
use crate::error::{GcpError, GcpResult};

const API: &str = "https://bigquery.googleapis.com/bigquery/v2";
const PREVIEW_ROWS: u32 = 50;
const MAX_ROWS: u32 = 500;
/// Queries bill at most this many bytes unless the caller allows more.
const DEFAULT_CAP: u64 = 10 * 1024 * 1024 * 1024;
const MAX_CAP: u64 = 1024 * 1024 * 1024 * 1024;
const QUERY_WAIT_MS: u64 = 30_000;
const MAX_POLLS: usize = 8;

fn number(value: &Value) -> Option<u64> {
    value
        .as_str()
        .and_then(|n| n.parse().ok())
        .or_else(|| value.as_u64())
}

fn fields_of(fields: &Value) -> Vec<Value> {
    fields
        .as_array()
        .into_iter()
        .flatten()
        .map(|field| {
            json!({
                "name": at(field, "/name"),
                "type": at(field, "/type"),
                "mode": field.get("mode").and_then(Value::as_str).unwrap_or("NULLABLE"),
                "description": at(field, "/description"),
                "fields": fields_of(at(field, "/fields")),
            })
        })
        .collect()
}

/// BigQuery's rows of `{ f: [{ v }] }` as objects keyed by column, nested records and arrays included.
pub fn rows_of(schema: &[Value], rows: &Value) -> Vec<Value> {
    rows.as_array()
        .into_iter()
        .flatten()
        .map(|row| record(schema, row))
        .collect()
}

fn record(schema: &[Value], row: &Value) -> Value {
    let cells = at(row, "/f").as_array().cloned().unwrap_or_default();
    let mut object = serde_json::Map::new();
    for (field, cell) in schema.iter().zip(cells.iter()) {
        object.insert(text(field, "/name"), cell_value(field, at(cell, "/v")));
    }
    Value::Object(object)
}

fn cell_value(field: &Value, value: &Value) -> Value {
    let nested: Vec<Value> = at(field, "/fields").as_array().cloned().unwrap_or_default();
    if text(field, "/mode") == "REPEATED" {
        let single = json!({ "name": at(field, "/name"), "type": at(field, "/type"), "fields": nested, "mode": "NULLABLE" });
        return Value::Array(
            value
                .as_array()
                .into_iter()
                .flatten()
                .map(|item| cell_value(&single, at(item, "/v")))
                .collect(),
        );
    }
    if !nested.is_empty() && value.is_object() {
        return record(&nested, value);
    }
    value.clone()
}

#[derive(Deserialize)]
pub struct Listing {
    #[serde(flatten)]
    pub target: Target,
}

/// The project's datasets with their location.
pub async fn datasets(input: Listing) -> GcpResult<Vec<Value>> {
    let project = input.target.project()?;
    let listed = client::list(
        &input.target,
        &format!("{API}/projects/{project}/datasets"),
        &[("maxResults", "1000".into())],
        "datasets",
        2000,
    )
    .await?;
    let mut read: Vec<Value> = listed
        .iter()
        .map(|dataset| json!({ "id": at(dataset, "/datasetReference/datasetId"), "location": at(dataset, "/location"), "name": at(dataset, "/friendlyName") }))
        .collect();
    read.sort_by_key(|each| text(each, "/id"));
    Ok(read)
}

#[derive(Deserialize)]
pub struct DatasetRef {
    #[serde(flatten)]
    pub target: Target,
    pub dataset: String,
}

/// A dataset's tables and views.
pub async fn tables(input: DatasetRef) -> GcpResult<Vec<Value>> {
    let project = input.target.project()?;
    let dataset = segment("dataset", &input.dataset)?;
    let listed = client::list(
        &input.target,
        &format!("{API}/projects/{project}/datasets/{dataset}/tables"),
        &[("maxResults", "1000".into())],
        "tables",
        2000,
    )
    .await?;
    let mut read: Vec<Value> = listed
        .iter()
        .map(|table| json!({ "id": at(table, "/tableReference/tableId"), "type": at(table, "/type"), "partitioned": table.get("timePartitioning").is_some() || table.get("rangePartitioning").is_some(), "createdAt": at(table, "/creationTime") }))
        .collect();
    read.sort_by_key(|each| text(each, "/id"));
    Ok(read)
}

#[derive(Deserialize)]
pub struct TableRef {
    #[serde(flatten)]
    pub target: Target,
    pub dataset: String,
    pub table: String,
}

fn table_url(input: &TableRef) -> GcpResult<String> {
    let project = input.target.project()?;
    Ok(format!(
        "{API}/projects/{project}/datasets/{}/tables/{}",
        segment("dataset", &input.dataset)?,
        segment("table", &input.table)?
    ))
}

/// A table's schema, size and layout, with its first rows when it is a table, which costs nothing to read.
pub async fn table(input: TableRef) -> GcpResult<Value> {
    let url = table_url(&input)?;
    let raw = client::get(&input.target, &url, &[]).await?;
    let schema = fields_of(at(&raw, "/schema/fields"));
    let kind = text(&raw, "/type");
    let preview = if kind == "TABLE" {
        let data = client::get(
            &input.target,
            &format!("{url}/data"),
            &[("maxResults", PREVIEW_ROWS.to_string())],
        )
        .await
        .unwrap_or(Value::Null);
        json!(rows_of(&schema, at(&data, "/rows")))
    } else {
        Value::Null
    };
    Ok(json!({
        "id": at(&raw, "/tableReference/tableId"),
        "type": kind,
        "description": at(&raw, "/description"),
        "rows": number(at(&raw, "/numRows")),
        "bytes": number(at(&raw, "/numBytes")),
        "location": at(&raw, "/location"),
        "modifiedAt": number(at(&raw, "/lastModifiedTime")),
        "partitionedBy": at(&raw, "/timePartitioning/field").as_str().or_else(|| at(&raw, "/timePartitioning/type").as_str()),
        "clusteredBy": at(&raw, "/clustering/fields"),
        "view": at(&raw, "/view/query"),
        "schema": schema,
        "preview": preview,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Query {
    #[serde(flatten)]
    pub target: Target,
    pub sql: String,
    /// Only estimate what the query would read, which is free.
    #[serde(default)]
    pub dry_run: bool,
    /// The most bytes the query may bill; it fails rather than read more. Default 10 GiB.
    #[serde(default)]
    pub max_bytes: Option<u64>,
}

/// What a query would cost at on-demand pricing, in US dollars, from the bytes it reads.
pub fn cost_of(bytes: u64) -> f64 {
    let tib = bytes as f64 / (1024.0 * 1024.0 * 1024.0 * 1024.0);
    (tib * 6.25 * 10_000.0).round() / 10_000.0
}

/// Runs standard SQL, or only estimates it, answering with its columns, rows and what it read.
pub async fn query(input: Query) -> GcpResult<Value> {
    let project = input.target.project()?;
    let sql = input.sql.trim();
    if sql.is_empty() {
        return Err(GcpError::BadArg("no query was given".into()));
    }
    let cap = input.max_bytes.unwrap_or(DEFAULT_CAP).min(MAX_CAP);
    let url = format!("{API}/projects/{project}/queries");
    let estimate = client::post(
        &input.target,
        &url,
        &json!({ "query": sql, "useLegacySql": false, "dryRun": true }),
    )
    .await?;
    let would_read = number(at(&estimate, "/totalBytesProcessed")).unwrap_or(0);
    if input.dry_run {
        return Ok(
            json!({ "dryRun": true, "bytes": would_read, "costUsd": cost_of(would_read), "schema": fields_of(at(&estimate, "/schema/fields")) }),
        );
    }
    if would_read > cap {
        return Err(GcpError::BadArg(format!(
            "the query would read {} GiB, more than the {} GiB it may bill; narrow it or allow more with maxBytes",
            would_read / (1024 * 1024 * 1024),
            cap / (1024 * 1024 * 1024)
        )));
    }
    let mut answer = client::post(
        &input.target,
        &url,
        &json!({ "query": sql, "useLegacySql": false, "maxResults": MAX_ROWS, "timeoutMs": QUERY_WAIT_MS, "maximumBytesBilled": cap.to_string() }),
    )
    .await?;
    for _ in 0..MAX_POLLS {
        if at(&answer, "/jobComplete").as_bool() != Some(false) {
            break;
        }
        let job = segment(
            "job",
            at(&answer, "/jobReference/jobId")
                .as_str()
                .unwrap_or_default(),
        )?;
        let mut query = vec![
            ("timeoutMs", QUERY_WAIT_MS.to_string()),
            ("maxResults", MAX_ROWS.to_string()),
        ];
        if let Some(location) = at(&answer, "/jobReference/location").as_str() {
            query.push(("location", location.to_string()));
        }
        answer = client::get(&input.target, &format!("{url}/{job}"), &query).await?;
    }
    if at(&answer, "/jobComplete").as_bool() == Some(false) {
        return Err(GcpError::Response(
            "the query is still running; look for it in BigQuery's job history".into(),
        ));
    }
    let schema = fields_of(at(&answer, "/schema/fields"));
    let billed = number(at(&answer, "/totalBytesBilled"))
        .or_else(|| number(at(&answer, "/totalBytesProcessed")))
        .unwrap_or(0);
    Ok(json!({
        "dryRun": false,
        "schema": schema,
        "rows": rows_of(&schema, at(&answer, "/rows")),
        "totalRows": number(at(&answer, "/totalRows")),
        "bytes": billed,
        "costUsd": cost_of(billed),
        "cached": at(&answer, "/cacheHit").as_bool() == Some(true),
        "job": at(&answer, "/jobReference/jobId"),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_read_as_objects_with_records_and_arrays() {
        let schema = fields_of(&json!([
            { "name": "id", "type": "INTEGER", "mode": "REQUIRED" },
            { "name": "tags", "type": "STRING", "mode": "REPEATED" },
            { "name": "address", "type": "RECORD", "fields": [{ "name": "city", "type": "STRING" }] }
        ]));
        let rows = rows_of(
            &schema,
            &json!([{ "f": [{ "v": "7" }, { "v": [{ "v": "vip" }, { "v": "eu" }] }, { "v": { "f": [{ "v": "Lisbon" }] } }] }]),
        );
        assert_eq!(
            rows[0],
            json!({ "id": "7", "tags": ["vip", "eu"], "address": { "city": "Lisbon" } })
        );
        assert_eq!(schema[0]["mode"], "REQUIRED");
        assert_eq!(schema[1]["mode"], "REPEATED");
        assert_eq!(schema[2]["fields"][0]["name"], "city");
    }

    #[test]
    fn cost_is_six_dollars_and_a_quarter_a_tebibyte() {
        assert_eq!(cost_of(0), 0.0);
        assert_eq!(cost_of(1024 * 1024 * 1024 * 1024), 6.25);
        assert_eq!(cost_of(10 * 1024 * 1024 * 1024), 0.061);
    }
}
