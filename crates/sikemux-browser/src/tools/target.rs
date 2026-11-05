//! Which point of a page an action means, from the element or place it names.

use super::*;

/// Draws the agent's pointer where it is about to act, so the person, and any
/// recording, can follow along. A page that refuses the drawing still acts.
pub(super) async fn show_pointer(view: &Tab, x: f64, y: f64, ripple: bool) {
    let _ = call(view, "mark", &[json!(x), json!(y), json!(ripple)]).await;
}

/// Where to point: the centre of the element a call names by number, `text`
/// or `selector`, scrolled into view, or CSS pixel coordinates in the tab's
/// viewport along with what lies under them.
pub(super) async fn target(
    view: &Tab,
    params: &Value,
    index_key: &str,
    x_key: &str,
    y_key: &str,
) -> Result<(f64, f64, Value), String> {
    let expected = params.get("expectLabel").cloned().unwrap_or(Value::Null);
    let named = element_target(params, index_key);
    if named != Value::Null {
        return point(view, named, expected).await;
    }
    match (
        params.get(x_key).and_then(Value::as_f64),
        params.get(y_key).and_then(Value::as_f64),
    ) {
        (Some(x), Some(y)) => {
            let hit = call(view, "hitAt", &[json!(x), json!(y), expected]).await?;
            Ok((x, y, hit))
        }
        _ => Err(format!(
            "pass {index_key}, text or selector, or both {x_key} and {y_key}"
        )),
    }
}

/// The element a call names, as the page script's `resolve` takes it, or
/// null when it names none and points by coordinates instead.
pub(super) fn element_target(params: &Value, index_key: &str) -> Value {
    let mut named = serde_json::Map::new();
    if let Some(index) = params.get(index_key).and_then(Value::as_u64) {
        named.insert("index".into(), json!(index));
    }
    if index_key == "index" {
        for key in ["text", "role", "selector"] {
            if let Some(value) = params.get(key).and_then(Value::as_str) {
                named.insert(key.into(), json!(value));
            }
        }
        if !named.contains_key("text") {
            named.remove("role");
        }
    }
    if named.is_empty() {
        Value::Null
    } else {
        Value::Object(named)
    }
}

/// The centre of the element `named` picks out, refused when `expected` no
/// longer matches its label. Text or a selector nothing matches yet is given
/// a moment to appear, as a page still drawing would show it.
pub(super) async fn point(
    view: &Tab,
    named: Value,
    expected: Value,
) -> Result<(f64, f64, Value), String> {
    let started = Instant::now();
    let point = loop {
        match call(view, "point", &[named.clone(), expected.clone()]).await {
            Err(error)
                if started.elapsed() < APPEAR_WAIT
                    && (error.starts_with("nothing shows or is labelled")
                        || error.starts_with("nothing shown matches")) =>
            {
                tokio::time::sleep(CONDITION_POLL).await;
            }
            answer => break answer?,
        }
    };
    let coordinate = |key: &str| {
        point
            .get(key)
            .and_then(Value::as_f64)
            .ok_or_else(|| "the page returned no position".to_string())
    };
    Ok((coordinate("x")?, coordinate("y")?, point))
}
