//! Compares the port against outputs captured from claude-agent-acp 0.81.2's own `tools.js`
//! (see `tests/fixtures/adapter_tools.json`).

use std::path::Path;

use super::super::tools;
use serde_json::Value;

fn fixture() -> Value {
    let text = include_str!("../fixtures/adapter_tools.json");
    serde_json::from_str(text).expect("fixture parses")
}

fn cases<'a>(fixture: &'a Value, key: &str) -> &'a Vec<Value> {
    fixture[key].as_array().expect("case list")
}

#[test]
fn tool_info_matches_adapter() {
    let fixture = fixture();
    let cwd = Path::new(fixture["cwd"].as_str().expect("cwd"));
    for case in cases(&fixture, "info") {
        let name = case["name"].as_str().expect("name");
        let input = &case["input"];
        let info = tools::tool_info(name, input, cwd);
        let expected = &case["info"];
        assert_eq!(info.title, expected["title"], "title of {name} {input}");
        assert_eq!(info.kind, expected["kind"], "kind of {name} {input}");
        assert_eq!(
            Value::Array(info.content),
            expected["content"],
            "content of {name} {input}"
        );
        assert_eq!(
            Value::Array(info.locations),
            expected["locations"],
            "locations of {name} {input}"
        );
        assert_eq!(
            tools::tool_call("toolu_1", name, input, cwd),
            case["tool_call"],
            "tool_call {name} {input}"
        );
        assert_eq!(
            tools::tool_call_refined("toolu_1", name, input, cwd),
            case["refined"],
            "refined {name} {input}"
        );
    }
}

#[test]
fn tool_results_match_adapter() {
    let fixture = fixture();
    let cwd = Path::new(fixture["cwd"].as_str().expect("cwd"));
    for case in cases(&fixture, "results") {
        let name = case["name"].as_str().expect("name");
        let tool_use_result = Some(&case["tool_use_result"]).filter(|v| !v.is_null());
        let actual = tools::tool_result(
            "toolu_1",
            name,
            &case["input"],
            &case["result"],
            tool_use_result,
            cwd,
        );
        assert_eq!(
            actual, case["expected"],
            "result of {name}: {}",
            case["result"]
        );
    }
}

#[test]
fn todo_plans_match_adapter() {
    let fixture = fixture();
    for case in cases(&fixture, "todos") {
        let actual = tools::todo_plan(&case["input"]).unwrap_or(Value::Null);
        assert_eq!(actual, case["expected"], "todo {}", case["input"]);
    }
}

#[test]
fn content_chunks_match_adapter() {
    let fixture = fixture();
    for case in cases(&fixture, "chunks") {
        let role = case["role"].as_str().expect("role");
        let actual = tools::content_chunk(&case["block"], role, case["message_id"].as_str())
            .unwrap_or(Value::Null);
        assert_eq!(actual, case["expected"], "chunk {}", case["block"]);
    }
}

#[test]
fn markdown_escape_matches_adapter() {
    let fixture = fixture();
    for case in cases(&fixture, "escape") {
        let text = case["text"].as_str().expect("text");
        assert_eq!(
            tools::markdown_escape(text),
            case["expected"],
            "escape {text:?}"
        );
    }
}
