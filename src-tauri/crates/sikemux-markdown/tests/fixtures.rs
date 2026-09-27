#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

//! The frontend tests draw markdown from blocks stored in a file, because
//! they cannot call this parser. This keeps those blocks what it really gives.

use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{json, Value};
use sikemux_markdown::{parse, Options};

#[derive(Deserialize)]
struct Fixture {
    text: String,
    options: Value,
    blocks: Value,
}

fn fixtures_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../src/test/markdownFixtures.json")
}

#[test]
fn the_frontend_fixtures_match_the_parser() {
    let path = fixtures_path();
    let file: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let fixtures: Vec<Fixture> = serde_json::from_value(file["cases"].clone()).unwrap();
    let update = std::env::var("SIKEMUX_MARKDOWN_FIXTURES").as_deref() == Ok("update");
    let mut stale = Vec::new();
    let mut cases = Vec::new();
    for fixture in fixtures {
        let options: Options = serde_json::from_value(fixture.options.clone()).unwrap();
        let blocks = serde_json::to_value(parse(&fixture.text, options, 0)).unwrap();
        if blocks != fixture.blocks {
            stale.push(fixture.text.clone());
        }
        cases.push(json!({ "text": fixture.text, "options": fixture.options, "blocks": blocks }));
    }
    if update {
        let text = serde_json::to_string_pretty(&json!({ "cases": cases })).unwrap();
        std::fs::write(&path, format!("{text}\n")).unwrap();
        return;
    }
    assert!(
        stale.is_empty(),
        "these fixtures differ from the parser; rerun with SIKEMUX_MARKDOWN_FIXTURES=update if the parser is right: {stale:#?}"
    );
}
