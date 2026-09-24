// SigNoz dashboards, read as they are saved and drawn by Sikemux. A panel's
// saved query goes back and forth untouched; this module turns it into a v5
// query when it runs, and turns the answer into one of three shapes a chart,
// a table or a single number can draw from.

use std::collections::BTreeMap;
use std::path::Path;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::client;
use crate::error::{SignozError, SignozResult};
use crate::filter::Scope;

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DashboardSummary {
    pub id: String,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub panels: usize,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Variable {
    pub name: String,
    pub options: Vec<String>,
    pub selected: String,
}

#[derive(Serialize, Debug, PartialEq, Clone, Copy, Default)]
#[serde(rename_all = "camelCase")]
pub struct Layout {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Panel {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub unit: String,
    pub layout: Layout,
    /// Whether Sikemux can draw it. Anything else links out to SigNoz.
    pub drawable: bool,
    /// The panel's saved query, handed back unchanged to run it.
    pub query: Value,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Dashboard {
    pub id: String,
    pub title: String,
    pub variables: Vec<Variable>,
    pub panels: Vec<Panel>,
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

pub async fn list(data_dir: &Path) -> SignozResult<Vec<DashboardSummary>> {
    let answer = client::request(data_dir, Method::GET, "/api/v1/dashboards", None).await?;
    let mut found: Vec<DashboardSummary> = answer
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let data = row.get("data")?;
            Some(DashboardSummary {
                id: row.get("id")?.as_str()?.to_string(),
                title: text(data, "title"),
                description: text(data, "description"),
                tags: data
                    .get("tags")
                    .and_then(Value::as_array)
                    .map(|tags| {
                        tags.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
                panels: data
                    .get("widgets")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len),
            })
        })
        .collect();
    found.sort_by_key(|dashboard| dashboard.title.to_lowercase());
    Ok(found)
}

#[derive(Deserialize)]
pub struct DashboardRequest {
    pub id: String,
}

fn checked_id(id: &str) -> SignozResult<&str> {
    let id = id.trim();
    if id.is_empty() || id.len() > 64 || !id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err(SignozError::BadArg("that is not a dashboard id".into()));
    }
    Ok(id)
}

pub async fn get(data_dir: &Path, request: DashboardRequest) -> SignozResult<Dashboard> {
    let id = checked_id(&request.id)?;
    let answer = client::request(
        data_dir,
        Method::GET,
        &format!("/api/v1/dashboards/{id}"),
        None,
    )
    .await?;
    let row = answer
        .get("data")
        .ok_or_else(|| SignozError::Response("no dashboard in the answer".into()))?;
    Ok(parse_dashboard(id, row.get("data").unwrap_or(&Value::Null)))
}

fn variables_of(data: &Value) -> Vec<Variable> {
    let mut variables: Vec<(u64, Variable)> = data
        .get("variables")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(Map::values)
        .filter_map(|variable| {
            let name = text(variable, "name");
            if name.is_empty() {
                return None;
            }
            let options: Vec<String> = text(variable, "customValue")
                .split(',')
                .map(str::trim)
                .filter(|option| !option.is_empty())
                .map(str::to_string)
                .collect();
            let selected = [
                text(variable, "selectedValue"),
                text(variable, "defaultValue"),
            ]
            .into_iter()
            .find(|value| !value.is_empty())
            .or_else(|| options.first().cloned())
            .unwrap_or_default();
            let order = variable
                .get("order")
                .and_then(Value::as_u64)
                .unwrap_or(u64::MAX);
            Some((
                order,
                Variable {
                    name,
                    options,
                    selected,
                },
            ))
        })
        .collect();
    variables.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.name.cmp(&right.1.name))
    });
    variables
        .into_iter()
        .map(|(_, variable)| variable)
        .collect()
}

fn drawable(kind: &str, query: &Value) -> bool {
    matches!(kind, "graph" | "bar" | "value" | "table" | "pie")
        && matches!(
            query.get("queryType").and_then(Value::as_str),
            Some("builder" | "clickhouse_sql")
        )
}

pub fn parse_dashboard(id: &str, data: &Value) -> Dashboard {
    let layouts: BTreeMap<String, Layout> = data
        .get("layout")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|cell| {
            let number = |key: &str| cell.get(key).and_then(Value::as_u64).unwrap_or(0) as u32;
            Some((
                cell.get("i")?.as_str()?.to_string(),
                Layout {
                    x: number("x"),
                    y: number("y"),
                    w: number("w"),
                    h: number("h"),
                },
            ))
        })
        .collect();
    let mut panels: Vec<Panel> = data
        .get("widgets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|widget| {
            let id = widget.get("id")?.as_str()?.to_string();
            let kind = text(widget, "panelTypes");
            let query = widget.get("query").cloned().unwrap_or(Value::Null);
            Some(Panel {
                layout: layouts.get(&id).copied().unwrap_or(Layout {
                    x: 0,
                    y: u32::MAX,
                    w: 6,
                    h: 6,
                }),
                title: text(widget, "title"),
                unit: text(widget, "yAxisUnit"),
                drawable: drawable(&kind, &query),
                kind,
                query,
                id,
            })
        })
        .collect();
    panels.sort_by_key(|panel| (panel.layout.y, panel.layout.x));
    Dashboard {
        id: id.to_string(),
        title: text(data, "title"),
        variables: variables_of(data),
        panels,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelRequest {
    pub kind: String,
    pub query: Value,
    #[serde(default)]
    pub variables: BTreeMap<String, String>,
    #[serde(flatten)]
    pub scope: Scope,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Series {
    pub label: String,
    /// Unix milliseconds and value.
    pub points: Vec<(u64, f64)>,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub name: String,
    pub aggregation: bool,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(tag = "shape", rename_all = "camelCase")]
pub enum PanelData {
    Series {
        series: Vec<Series>,
    },
    Table {
        columns: Vec<Column>,
        rows: Vec<Vec<Value>>,
    },
    Value {
        value: Option<f64>,
    },
}

fn is_disabled(value: &Value) -> bool {
    value
        .get("disabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn builder_spec(query: &Value) -> Option<Value> {
    let mut spec = Map::new();
    spec.insert("name".into(), json!(text(query, "queryName")));
    spec.insert("signal".into(), json!(text(query, "dataSource")));
    spec.insert("disabled".into(), json!(is_disabled(query)));
    spec.insert(
        "aggregations".into(),
        query
            .get("aggregations")
            .cloned()
            .unwrap_or_else(|| json!([])),
    );
    let expression = |key: &str| {
        query
            .pointer(&format!("/{key}/expression"))
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string()
    };
    let filter = expression("filter");
    if !filter.is_empty() {
        spec.insert("filter".into(), json!({ "expression": filter }));
    }
    let having = expression("having");
    if !having.is_empty() {
        spec.insert("having".into(), json!({ "expression": having }));
    }
    let group_by: Vec<Value> = query
        .get("groupBy")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|group| json!({ "name": text(group, "key"), "fieldContext": text(group, "type"), "fieldDataType": text(group, "dataType") }))
        .collect();
    if !group_by.is_empty() {
        spec.insert("groupBy".into(), json!(group_by));
    }
    let order: Vec<Value> = query
        .get("orderBy")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|order| json!({ "key": { "name": text(order, "columnName") }, "direction": if text(order, "order").is_empty() { "desc".to_string() } else { text(order, "order") } }))
        .collect();
    if !order.is_empty() {
        spec.insert("order".into(), json!(order));
    }
    for key in ["stepInterval", "limit"] {
        if let Some(value) = query.get(key).filter(|value| value.is_number()) {
            spec.insert(key.into(), value.clone());
        }
    }
    for key in ["legend", "functions"] {
        if let Some(value) = query
            .get(key)
            .filter(|value| !value.is_null() && **value != json!("") && **value != json!([]))
        {
            spec.insert(key.into(), value.clone());
        }
    }
    Some(Value::Object(spec))
}

/// The saved query in the form v5 runs, which is how dashboards are stored
/// once SigNoz has migrated them.
pub fn queries_of(saved: &Value) -> SignozResult<Vec<Value>> {
    let queries: Vec<Value> = match saved.get("queryType").and_then(Value::as_str) {
        Some("builder") => {
            let builder = saved.get("builder").unwrap_or(&Value::Null);
            let data = builder.get("queryData").and_then(Value::as_array).into_iter().flatten();
            let formulas = builder.get("queryFormulas").and_then(Value::as_array).into_iter().flatten();
            data.filter_map(|query| builder_spec(query).map(|spec| json!({ "type": "builder_query", "spec": spec })))
                .chain(formulas.map(|formula| {
                    json!({ "type": "builder_formula", "spec": {
                        "name": text(formula, "queryName"),
                        "expression": text(formula, "expression"),
                        "disabled": is_disabled(formula),
                        "legend": text(formula, "legend"),
                    } })
                }))
                .collect()
        }
        Some("clickhouse_sql") => saved
            .get("clickhouse_sql")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|query| !text(query, "query").is_empty())
            .map(|query| json!({ "type": "clickhouse_sql", "spec": { "name": text(query, "name"), "query": text(query, "query"), "disabled": is_disabled(query) } }))
            .collect(),
        _ => Vec::new(),
    };
    if queries.is_empty() {
        return Err(SignozError::BadArg(
            "Sikemux cannot run this panel's query; open it in SigNoz".into(),
        ));
    }
    Ok(queries)
}

fn request_type(kind: &str) -> SignozResult<&'static str> {
    match kind {
        "graph" | "bar" => Ok("time_series"),
        "value" | "table" | "pie" => Ok("scalar"),
        _ => Err(SignozError::BadArg(format!(
            "Sikemux cannot draw a {kind} panel; open it in SigNoz"
        ))),
    }
}

/// One panel of a saved dashboard, found by id, so a caller never handles the
/// saved query itself. Variables not given keep the dashboard's own choice.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedPanelRequest {
    pub dashboard_id: String,
    pub panel_id: String,
    #[serde(default, deserialize_with = "crate::filter::plain_values")]
    pub variables: BTreeMap<String, String>,
    #[serde(flatten)]
    pub scope: Scope,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedPanel {
    pub title: String,
    pub kind: String,
    pub unit: String,
    pub data: PanelData,
}

pub async fn saved_panel(data_dir: &Path, request: SavedPanelRequest) -> SignozResult<SavedPanel> {
    let dashboard = get(
        data_dir,
        DashboardRequest {
            id: request.dashboard_id,
        },
    )
    .await?;
    let panel = dashboard
        .panels
        .into_iter()
        .find(|panel| panel.id == request.panel_id)
        .ok_or_else(|| SignozError::BadArg("that dashboard has no panel with this id".into()))?;
    if !panel.drawable {
        return Err(SignozError::BadArg(format!(
            "`{}` is a {} panel, which Sikemux cannot read yet",
            panel.title, panel.kind
        )));
    }
    let mut variables: BTreeMap<String, String> = dashboard
        .variables
        .into_iter()
        .map(|variable| (variable.name, variable.selected))
        .collect();
    variables.extend(request.variables);
    let data = self::panel(
        data_dir,
        PanelRequest {
            kind: panel.kind.clone(),
            query: panel.query,
            variables,
            scope: request.scope,
        },
    )
    .await?;
    Ok(SavedPanel {
        title: panel.title,
        kind: panel.kind,
        unit: panel.unit,
        data,
    })
}

pub async fn panel(data_dir: &Path, request: PanelRequest) -> SignozResult<PanelData> {
    let request_type = request_type(&request.kind)?;
    let (start, end) = request.scope.window()?;
    let variables: Map<String, Value> = request
        .variables
        .iter()
        .map(|(name, value)| (name.clone(), json!({ "type": "custom", "value": value })))
        .collect();
    let body = json!({
        "schemaVersion": "v1",
        "start": start,
        "end": end,
        "requestType": request_type,
        "variables": variables,
        "compositeQuery": { "queries": queries_of(&request.query)? },
    });
    let answer =
        client::request(data_dir, Method::POST, "/api/v5/query_range", Some(&body)).await?;
    let results = answer
        .pointer("/data/data/results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(match request.kind.as_str() {
        "graph" | "bar" => PanelData::Series {
            series: series_of(&results, &request.query),
        },
        "value" => PanelData::Value {
            value: value_of(&results),
        },
        _ => table_of(&results),
    })
}

fn legend_of(saved: &Value, query_name: &str) -> String {
    let lists = [
        "/builder/queryData",
        "/builder/queryFormulas",
        "/clickhouse_sql",
    ];
    lists
        .iter()
        .filter_map(|pointer| saved.pointer(pointer).and_then(Value::as_array))
        .flatten()
        .find(|query| text(query, "queryName") == query_name || text(query, "name") == query_name)
        .map(|query| text(query, "legend"))
        .unwrap_or_default()
}

/// A legend like `{{service.name}} p99` filled from the series' labels, or
/// the labels themselves, or the query's name.
fn label_for(legend: &str, labels: &[(String, String)], query_name: &str) -> String {
    if !legend.is_empty() {
        let mut label = legend.to_string();
        for (key, value) in labels {
            label = label.replace(&format!("{{{{{key}}}}}"), value);
        }
        return label;
    }
    if labels.is_empty() {
        return query_name.to_string();
    }
    labels
        .iter()
        .map(|(_, value)| value.as_str())
        .collect::<Vec<_>>()
        .join(" · ")
}

fn series_of(results: &[Value], saved: &Value) -> Vec<Series> {
    let mut out = Vec::new();
    for result in results {
        let query_name = text(result, "queryName");
        let legend = legend_of(saved, &query_name);
        for aggregation in result
            .get("aggregations")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            for serie in aggregation
                .get("series")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let labels: Vec<(String, String)> = serie
                    .get("labels")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|label| {
                        let key = label
                            .pointer("/key/name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        let value = match label.get("value") {
                            Some(Value::String(text)) => text.clone(),
                            Some(other) => other.to_string(),
                            None => String::new(),
                        };
                        (key, value)
                    })
                    .collect();
                let points = serie
                    .get("values")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|point| {
                        Some((
                            point.get("timestamp")?.as_u64()?,
                            point.get("value")?.as_f64()?,
                        ))
                    })
                    .collect();
                out.push(Series {
                    label: label_for(&legend, &labels, &query_name),
                    points,
                });
            }
        }
    }
    out
}

fn table_of(results: &[Value]) -> PanelData {
    let Some(result) = results
        .iter()
        .find(|result| result.get("columns").is_some())
    else {
        return PanelData::Table {
            columns: Vec::new(),
            rows: Vec::new(),
        };
    };
    let columns = result
        .get("columns")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|column| Column {
            name: text(column, "name"),
            aggregation: text(column, "columnType") == "aggregation",
        })
        .collect();
    let rows = result
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| row.as_array().cloned())
        .collect();
    PanelData::Table { columns, rows }
}

/// The last aggregation column of the first row: with a formula, that is the formula.
fn value_of(results: &[Value]) -> Option<f64> {
    let PanelData::Table { columns, rows } = table_of(results) else {
        return None;
    };
    let column = columns.iter().rposition(|column| column.aggregation)?;
    rows.first()?.get(column)?.as_f64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_panels_in_layout_order_with_their_variables() {
        let data = json!({
            "title": "Render farm",
            "variables": { "v1": { "name": "environment", "customValue": "dev, production", "selectedValue": "dev", "order": 0 } },
            "layout": [{ "i": "b", "x": 6, "y": 0, "w": 6, "h": 3 }, { "i": "a", "x": 0, "y": 0, "w": 6, "h": 3 }],
            "widgets": [
                { "id": "b", "title": "Queue", "panelTypes": "value", "query": { "queryType": "builder" } },
                { "id": "a", "title": "Jobs", "panelTypes": "graph", "yAxisUnit": "ms", "query": { "queryType": "clickhouse_sql" } },
                { "id": "c", "title": "Spread", "panelTypes": "histogram", "query": { "queryType": "builder" } },
            ],
        });
        let dashboard = parse_dashboard("d1", &data);
        assert_eq!(
            dashboard
                .panels
                .iter()
                .map(|panel| panel.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
        assert!(dashboard.panels[0].drawable && !dashboard.panels[2].drawable);
        assert_eq!(
            dashboard.variables,
            [Variable {
                name: "environment".into(),
                options: vec!["dev".into(), "production".into()],
                selected: "dev".into()
            }]
        );
    }

    #[test]
    fn turns_a_saved_builder_query_into_v5() {
        let saved = json!({ "queryType": "builder", "builder": {
            "queryData": [{
                "queryName": "A", "dataSource": "traces", "disabled": false,
                "aggregations": [{ "expression": "count()" }],
                "filter": { "expression": "service.name = $service" },
                "having": { "expression": "" },
                "groupBy": [{ "key": "service.name", "type": "resource", "dataType": "string" }],
                "orderBy": [], "legend": "{{service.name}}", "limit": null, "stepInterval": null, "functions": [],
            }],
            "queryFormulas": [{ "queryName": "F1", "expression": "A * 2", "disabled": false }],
        } });
        let queries = queries_of(&saved).unwrap();
        assert_eq!(
            queries[0]["spec"]["filter"]["expression"],
            "service.name = $service"
        );
        assert_eq!(
            queries[0]["spec"]["groupBy"][0],
            json!({ "name": "service.name", "fieldContext": "resource", "fieldDataType": "string" })
        );
        assert!(
            queries[0]["spec"].get("having").is_none() && queries[0]["spec"].get("limit").is_none()
        );
        assert_eq!(queries[1]["type"], "builder_formula");
        assert!(queries_of(&json!({ "queryType": "promql" })).is_err());
    }

    #[test]
    fn names_series_from_the_legend_or_their_labels() {
        let labels = vec![("service.name".to_string(), "api".to_string())];
        assert_eq!(label_for("{{service.name}} p99", &labels, "A"), "api p99");
        assert_eq!(label_for("", &labels, "A"), "api");
        assert_eq!(label_for("", &[], "A"), "A");
    }

    #[test]
    fn takes_a_single_number_from_the_last_aggregation() {
        let results = vec![json!({
            "columns": [{ "name": "service.name", "columnType": "group" }, { "name": "A", "columnType": "aggregation" }, { "name": "F1", "columnType": "aggregation" }],
            "data": [["api", 3, 6.5]],
        })];
        assert_eq!(value_of(&results), Some(6.5));
        assert_eq!(value_of(&[]), None);
    }
}
