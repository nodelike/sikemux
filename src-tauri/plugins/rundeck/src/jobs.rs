// A job's definition as the run form needs it, and the values list a job
// option can load from a remote URL.

use std::time::Duration;

use futures::StreamExt;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::error::{RundeckError, RundeckResult};

use crate::client::{get_json, get_json_at_version, seg, JOB_DEFINITION_API_VERSION};
use crate::config;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct JobDetail {
    pub id: String,
    pub name: String,
    pub group: Option<String>,
    pub project: String,
    pub description: Option<String>,
    pub execution_enabled: bool,
    pub schedule_enabled: bool,
    pub scheduled: bool,
    pub node_filter: Option<String>,
    pub options: Vec<JobOption>,
    pub steps: Vec<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct JobOption {
    pub name: String,
    pub label: Option<String>,
    pub description: Option<String>,
    pub required: bool,
    pub secure: bool,
    pub value_exposed: bool,
    pub default: Option<String>,
    pub values: Option<Vec<String>>,
    pub values_url: Option<String>,
    pub enforced: bool,
    pub multivalued: bool,
    pub delimiter: Option<String>,
    pub is_date: bool,
    pub date_format: Option<String>,
    pub kind: &'static str,
}

fn text(value: &Value, key: &str) -> Option<String> {
    match value.get(key)? {
        Value::String(text) if !text.trim().is_empty() => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn flag(value: &Value, key: &str) -> bool {
    match value.get(key) {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::String(text)) => text.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

fn allowed_values(option: &Value) -> Option<Vec<String>> {
    match option.get("values")? {
        Value::Array(items) => Some(
            items
                .iter()
                .filter_map(|item| match item {
                    Value::String(text) => Some(text.clone()),
                    Value::Null => None,
                    other => Some(other.to_string()),
                })
                .collect(),
        ),
        Value::String(list) => Some(
            list.split(',')
                .map(|item| item.trim().to_string())
                .collect(),
        ),
        _ => None,
    }
}

fn parse_option(name: Option<String>, option: &Value) -> Option<JobOption> {
    let name = text(option, "name").or(name)?;
    Some(JobOption {
        name,
        label: text(option, "label"),
        description: text(option, "description"),
        required: flag(option, "required"),
        secure: flag(option, "secure"),
        value_exposed: flag(option, "valueExposed"),
        default: text(option, "value"),
        values: allowed_values(option),
        values_url: text(option, "valuesUrl"),
        enforced: flag(option, "enforced"),
        multivalued: flag(option, "multivalued"),
        delimiter: text(option, "delimiter"),
        is_date: flag(option, "isDate"),
        date_format: text(option, "dateFormat"),
        kind: if text(option, "type").as_deref() == Some("file") {
            "file"
        } else {
            "text"
        },
    })
}

fn parse_options(options: Option<&Value>) -> Vec<JobOption> {
    match options {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|option| parse_option(None, option))
            .collect(),
        Some(Value::Object(map)) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by_key(|(_, option)| {
                option
                    .get("sortIndex")
                    .and_then(Value::as_i64)
                    .unwrap_or(i64::MAX)
            });
            entries
                .into_iter()
                .filter_map(|(name, option)| parse_option(Some(name.clone()), option))
                .collect()
        }
        _ => Vec::new(),
    }
}

fn step_label(step: &Value) -> String {
    if let Some(label) = text(step, "description").or_else(|| text(step, "exec")) {
        return label;
    }
    if ["script", "scriptfile", "scripturl"]
        .iter()
        .any(|key| text(step, key).is_some())
    {
        return "script".into();
    }
    if let Some(job) = step.get("jobref") {
        let name = text(job, "name").unwrap_or_default();
        return match text(job, "group") {
            Some(group) => format!("job: {group}/{name}"),
            None => format!("job: {name}"),
        };
    }
    text(step, "type").unwrap_or_else(|| "step".into())
}

fn parse_job_detail(definition: &Value) -> RundeckResult<JobDetail> {
    let job = match definition {
        Value::Array(items) => items.first(),
        other => Some(other),
    }
    .ok_or_else(|| RundeckError::Api("Rundeck returned no job definition".into()))?;
    let steps = job
        .pointer("/sequence/commands")
        .and_then(Value::as_array)
        .map(|commands| commands.iter().map(step_label).collect())
        .unwrap_or_default();
    Ok(JobDetail {
        id: text(job, "id")
            .or_else(|| text(job, "uuid"))
            .unwrap_or_default(),
        name: text(job, "name").unwrap_or_default(),
        group: text(job, "group"),
        project: text(job, "project").unwrap_or_default(),
        description: text(job, "description"),
        execution_enabled: job.get("executionEnabled").is_none() || flag(job, "executionEnabled"),
        schedule_enabled: job.get("scheduleEnabled").is_none() || flag(job, "scheduleEnabled"),
        scheduled: job
            .get("schedule")
            .is_some_and(|schedule| !schedule.is_null()),
        node_filter: job
            .pointer("/nodefilters/filter")
            .and_then(|filter| match filter {
                Value::String(filter) if !filter.trim().is_empty() => Some(filter.clone()),
                _ => None,
            }),
        options: parse_options(job.get("options")),
        steps,
    })
}

pub async fn job_detail(job_id: String) -> RundeckResult<JobDetail> {
    let id = seg(&job_id)?;
    let definition: Value = get_json_at_version(
        JOB_DEFINITION_API_VERSION,
        &format!("/job/{id}"),
        &[("format", "json".into())],
    )
    .await?;
    let mut detail = parse_job_detail(&definition)?;
    if detail.id.is_empty() {
        detail.id = job_id;
    }
    if detail.project.is_empty() {
        let info: Value = get_json(&format!("/job/{id}/info"), &[]).await?;
        detail.project = text(&info, "project").unwrap_or_default();
    }
    Ok(detail)
}

// ---- remote option values -------------------------------------------------

const OPTION_VALUES_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_OPTION_VALUES_BYTES: usize = 1024 * 1024;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct OptionValue {
    pub name: String,
    pub value: String,
}

fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn parse_option_values(body: &Value) -> RundeckResult<Vec<OptionValue>> {
    let pair = |name: String, value: String| OptionValue { name, value };
    match body {
        Value::Array(items) => Ok(items
            .iter()
            .filter_map(|item| match item {
                Value::Object(entry) => {
                    let value = entry.get("value").and_then(scalar_text);
                    let name = entry.get("name").and_then(scalar_text);
                    match (name, value) {
                        (Some(name), Some(value)) => Some(pair(name, value)),
                        (None, Some(value)) => Some(pair(value.clone(), value)),
                        (Some(name), None) => Some(pair(name.clone(), name)),
                        (None, None) => None,
                    }
                }
                other => scalar_text(other).map(|value| pair(value.clone(), value)),
            })
            .collect()),
        Value::Object(map) => Ok(object_pairs(map)),
        _ => Err(RundeckError::Api(
            "option values must be a JSON list or object".into(),
        )),
    }
}

fn object_pairs(map: &Map<String, Value>) -> Vec<OptionValue> {
    map.iter()
        .filter_map(|(name, value)| {
            scalar_text(value).map(|value| OptionValue {
                name: name.clone(),
                value,
            })
        })
        .collect()
}

pub async fn option_values(url: String) -> RundeckResult<Vec<OptionValue>> {
    let transport = config::validate_transport(&url, true).await?;
    let client = transport
        .pin_dns(reqwest::Client::builder())
        .timeout(OPTION_VALUES_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("sikemux-rundeck/0.1")
        .build()?;
    let resp = client
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(RundeckError::Http {
            status: status.as_u16(),
            message: format!("option values URL answered {status}"),
        });
    }
    if resp
        .content_length()
        .is_some_and(|size| size > MAX_OPTION_VALUES_BYTES as u64)
    {
        return Err(RundeckError::Api("option values exceed 1 MiB".into()));
    }
    let mut stream = resp.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if bytes.len() + chunk.len() > MAX_OPTION_VALUES_BYTES {
            return Err(RundeckError::Api("option values exceed 1 MiB".into()));
        }
        bytes.extend_from_slice(&chunk);
    }
    let body: Value = serde_json::from_slice(&bytes)
        .map_err(|_| RundeckError::Api("option values URL did not return JSON".into()))?;
    parse_option_values(&body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn definition(options: Value) -> Value {
        json!([{
            "id": "abc",
            "name": "deploy",
            "group": "backend",
            "project": "ops",
            "description": "",
            "executionEnabled": false,
            "scheduleEnabled": true,
            "schedule": {"time": {"hour": "1"}},
            "nodefilters": {"filter": "tags: web"},
            "options": options,
            "sequence": {"commands": [
                {"description": "Build", "exec": "make"},
                {"exec": "echo hi"},
                {"scriptfile": "/tmp/x.sh"},
                {"jobref": {"group": "lib", "name": "notify"}},
                {"jobref": {"name": "cleanup"}},
                {"type": "com.example.Plugin", "nodeStep": true},
                {}
            ]}
        }])
    }

    #[test]
    fn reads_options_in_definition_order_from_a_list() {
        let detail = parse_job_detail(&definition(json!([
            {"name": "BRANCH", "required": true, "value": "main", "values": ["main", "dev"], "enforced": true},
            {"name": "CONFIG", "type": "file", "secure": true, "valueExposed": true},
            {"name": "WHEN", "isDate": true, "dateFormat": "YYYY", "valuesUrl": "https://x/values", "multivalued": true, "delimiter": ","}
        ])))
        .unwrap();
        assert_eq!(detail.id, "abc");
        assert_eq!(detail.description, None);
        assert!(!detail.execution_enabled);
        assert!(detail.scheduled);
        assert_eq!(detail.node_filter.as_deref(), Some("tags: web"));
        let names: Vec<_> = detail.options.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(names, ["BRANCH", "CONFIG", "WHEN"]);
        let branch = &detail.options[0];
        assert!(branch.required && branch.enforced);
        assert_eq!(branch.default.as_deref(), Some("main"));
        assert_eq!(
            branch.values.as_deref(),
            Some(&["main".to_string(), "dev".to_string()][..])
        );
        assert_eq!(detail.options[1].kind, "file");
        assert!(detail.options[1].secure && detail.options[1].value_exposed);
        assert!(detail.options[2].is_date && detail.options[2].multivalued);
        assert_eq!(
            detail.options[2].values_url.as_deref(),
            Some("https://x/values")
        );
    }

    #[test]
    fn reads_options_from_a_name_keyed_map() {
        let detail = parse_job_detail(&definition(json!({
            "BRANCH": {"value": "main"},
            "TAG": {"name": "TAG", "values": "a, b"}
        })))
        .unwrap();
        let names: Vec<_> = detail.options.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(names, ["BRANCH", "TAG"]);
        assert_eq!(
            detail.options[1].values.as_deref(),
            Some(&["a".to_string(), "b".to_string()][..])
        );
        assert_eq!(detail.options[0].kind, "text");
    }

    #[test]
    fn labels_each_step() {
        let detail = parse_job_detail(&definition(json!([]))).unwrap();
        assert_eq!(
            detail.steps,
            [
                "Build",
                "echo hi",
                "script",
                "job: lib/notify",
                "job: cleanup",
                "com.example.Plugin",
                "step"
            ]
        );
    }

    #[test]
    fn missing_flags_default_to_enabled_and_unscheduled() {
        let detail = parse_job_detail(&json!([{"id": "x", "name": "n"}])).unwrap();
        assert!(detail.execution_enabled && detail.schedule_enabled);
        assert!(!detail.scheduled);
        assert!(detail.options.is_empty() && detail.steps.is_empty());
    }

    #[test]
    fn option_values_accept_strings_pairs_and_maps() {
        let pair = |name: &str, value: &str| OptionValue {
            name: name.into(),
            value: value.into(),
        };
        assert_eq!(
            parse_option_values(&json!(["a", "b"])).unwrap(),
            [pair("a", "a"), pair("b", "b")]
        );
        assert_eq!(
            parse_option_values(&json!([{"name": "Main", "value": "main"}])).unwrap(),
            [pair("Main", "main")]
        );
        assert_eq!(
            parse_option_values(&json!({"Main": "main"})).unwrap(),
            [pair("Main", "main")]
        );
        assert!(parse_option_values(&json!("nope")).is_err());
    }
}
