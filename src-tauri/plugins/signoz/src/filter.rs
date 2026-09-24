use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer};
use serde_json::Value;

use crate::error::{SignozError, SignozResult};
use crate::query::{self, quote};

const MAX_WINDOW_MS: u64 = 7 * 24 * 60 * 60 * 1_000;

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FilterOp {
    Equals,
    NotEquals,
    Contains,
    NotContains,
    Exists,
    NotExists,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Filter {
    pub key: String,
    pub op: FilterOp,
    #[serde(default, deserialize_with = "plain_value")]
    pub value: String,
}

/// A value as written in a filter or a dashboard variable. Numbers and booleans
/// are read as their text, since SigNoz compares `status = '503'` and `status = 503` alike.
fn text_of(value: Value) -> Result<String, String> {
    match value {
        Value::String(text) => Ok(text),
        Value::Number(number) => Ok(number.to_string()),
        Value::Bool(flag) => Ok(flag.to_string()),
        Value::Null => Ok(String::new()),
        other => Err(format!("expected text or a number, not {other}")),
    }
}

pub fn plain_value<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    text_of(Value::deserialize(deserializer)?).map_err(serde::de::Error::custom)
}

pub fn plain_values<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, String>, D::Error> {
    BTreeMap::<String, Value>::deserialize(deserializer)?
        .into_iter()
        .map(|(name, value)| {
            text_of(value)
                .map(|text| (name.clone(), text))
                .map_err(|problem| serde::de::Error::custom(format!("`{name}`: {problem}")))
        })
        .collect()
}

/// What every query narrows by, whichever signal it reads.
#[derive(Deserialize, Default, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Scope {
    pub service: Option<String>,
    pub environment: Option<String>,
    #[serde(default)]
    pub filters: Vec<Filter>,
    /// A filter in SigNoz's own query syntax, for anything the others cannot say.
    pub expression: Option<String>,
    /// A fixed range in Unix milliseconds. Without one, the last `minutes`.
    pub start: Option<u64>,
    pub end: Option<u64>,
    pub minutes: Option<u32>,
}

/// Each part of a query is bracketed and joined with AND, so an expression with
/// a stray closing bracket could step outside its own part and undo the rest.
fn balanced(expression: &str) -> SignozResult<&str> {
    let mut depth = 0_i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in expression.chars() {
        if let Some(open) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == open {
                quote = None;
            }
            continue;
        }
        match c {
            '\'' | '"' => quote = Some(c),
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth < 0 {
                    break;
                }
            }
            _ => {}
        }
    }
    if depth != 0 || quote.is_some() {
        return Err(SignozError::BadArg(
            "the expression's brackets or quotes do not balance".into(),
        ));
    }
    Ok(expression)
}

/// Attribute names are written into the expression as they are, so only
/// characters a name can have get through.
fn checked_key(key: &str) -> SignozResult<&str> {
    let key = key.trim();
    let mut chars = key.chars();
    let starts_well = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '@');
    let rest_ok =
        chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | ':' | '@' | '/'));
    if key.len() > 200 || !starts_well || !rest_ok {
        return Err(SignozError::BadArg(format!(
            "`{key}` is not an attribute name"
        )));
    }
    Ok(key)
}

pub fn clause(filter: &Filter) -> SignozResult<String> {
    let key = checked_key(&filter.key)?;
    let value = quote(&filter.value);
    Ok(match filter.op {
        FilterOp::Equals => format!("{key} = {value}"),
        FilterOp::NotEquals => format!("{key} != {value}"),
        FilterOp::Contains => format!("{key} CONTAINS {value}"),
        FilterOp::NotContains => format!("{key} NOT CONTAINS {value}"),
        FilterOp::Exists => format!("{key} EXISTS"),
        FilterOp::NotExists => format!("NOT ({key} EXISTS)"),
    })
}

fn present(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

impl Scope {
    pub fn clauses(&self) -> SignozResult<Vec<String>> {
        let mut clauses = Vec::new();
        if let Some(service) = present(&self.service) {
            clauses.push(format!("service.name = {}", quote(service)));
        }
        if let Some(environment) = present(&self.environment) {
            clauses.push(format!("deployment.environment = {}", quote(environment)));
        }
        for filter in &self.filters {
            clauses.push(clause(filter)?);
        }
        if let Some(expression) = present(&self.expression) {
            clauses.push(balanced(expression)?.to_string());
        }
        Ok(clauses)
    }

    /// A fixed range, or the last `minutes`. A range that runs backwards, has
    /// only one end, or spans more than a week is refused rather than replaced.
    pub fn window(&self) -> SignozResult<(u64, u64)> {
        match (self.start, self.end) {
            (None, None) => Ok(query::window(self.minutes)),
            (Some(start), Some(end)) if start >= end => Err(SignozError::BadArg(
                "the window's start must come before its end".into(),
            )),
            (Some(start), Some(end)) if end - start > MAX_WINDOW_MS => Err(SignozError::BadArg(
                "a window can span at most 7 days".into(),
            )),
            (Some(start), Some(end)) => Ok((start, end)),
            _ => Err(SignozError::BadArg(
                "give both start and end, or neither".into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(key: &str, op: FilterOp, value: &str) -> Filter {
        Filter {
            key: key.into(),
            op,
            value: value.into(),
        }
    }

    #[test]
    fn writes_each_operator_in_signoz_syntax() {
        assert_eq!(
            clause(&filter("path", FilterOp::Equals, "/a")).unwrap(),
            "path = '/a'"
        );
        assert_eq!(
            clause(&filter("path", FilterOp::NotEquals, "/a")).unwrap(),
            "path != '/a'"
        );
        assert_eq!(
            clause(&filter("body", FilterOp::NotContains, "it's")).unwrap(),
            "body NOT CONTAINS 'it\\'s'"
        );
        assert_eq!(
            clause(&filter("code.file.path", FilterOp::Exists, "")).unwrap(),
            "code.file.path EXISTS"
        );
        assert_eq!(
            clause(&filter("user_id", FilterOp::NotExists, "")).unwrap(),
            "NOT (user_id EXISTS)"
        );
    }

    #[test]
    fn refuses_names_that_could_change_the_query() {
        for key in ["path = 'x' OR 1", "a b", "", "'quoted'", "(path)", "9lives"] {
            assert!(
                clause(&filter(key, FilterOp::Equals, "x")).is_err(),
                "{key} should be refused"
            );
        }
    }

    #[test]
    fn scopes_by_service_environment_filters_and_expression() {
        let scope = Scope {
            service: Some("api".into()),
            environment: Some("production".into()),
            filters: vec![filter("status", FilterOp::Equals, "503")],
            expression: Some("latency_ms > 100".into()),
            ..Scope::default()
        };
        assert_eq!(
            scope.clauses().unwrap(),
            [
                "service.name = 'api'",
                "deployment.environment = 'production'",
                "status = '503'",
                "latency_ms > 100"
            ]
        );
    }

    #[test]
    fn keeps_a_fixed_range_and_refuses_one_it_cannot_honour() {
        let range = |start: Option<u64>, end: Option<u64>| Scope {
            start,
            end,
            minutes: Some(5),
            ..Scope::default()
        };
        assert_eq!(
            range(Some(1_000), Some(5_000)).window().unwrap(),
            (1_000, 5_000)
        );
        let (start, end) = range(None, None).window().unwrap();
        assert_eq!(end - start, 5 * 60_000);
        assert!(range(Some(5_000), Some(1_000)).window().is_err());
        assert!(range(Some(1_000), None).window().is_err());
        assert!(range(None, Some(1_000)).window().is_err());
        assert!(range(Some(0), Some(MAX_WINDOW_MS + 1)).window().is_err());
    }

    #[test]
    fn an_expression_cannot_close_the_bracket_around_it() {
        let scope = |expression: &str| Scope {
            service: Some("api".into()),
            expression: Some(expression.into()),
            ..Scope::default()
        };
        assert!(scope("x = 1) OR (1 = 1").clauses().is_err());
        assert!(scope("(a = 1").clauses().is_err());
        assert!(scope("path = 'it''s").clauses().is_err());
        assert!(scope("path = ')' AND (a = 1 OR b = 2)").clauses().is_ok());
    }

    #[test]
    fn reads_numbers_and_booleans_as_filter_values() {
        let filters: Vec<Filter> = serde_json::from_value(serde_json::json!([
            { "key": "status", "op": "equals", "value": 503 },
            { "key": "cached", "op": "equals", "value": true },
            { "key": "path", "op": "exists" },
        ]))
        .unwrap();
        let values: Vec<&str> = filters.iter().map(|filter| filter.value.as_str()).collect();
        assert_eq!(values, ["503", "true", ""]);
    }
}
