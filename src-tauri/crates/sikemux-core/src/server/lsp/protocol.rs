use serde_json::Value;
use url::Url;

use super::limits::MAX_LSP_PATH_BYTES;
use super::types::{LspDocumentSymbol, LspLocation, LspRange};

const MAX_DOCUMENT_SYMBOLS: usize = 2_000;
const MAX_DOCUMENT_SYMBOL_DEPTH: usize = 16;
const MAX_SYMBOL_NAME_BYTES: usize = 256;
const MAX_SYMBOL_DETAIL_BYTES: usize = 1_024;

pub(super) fn path_to_uri(path: &str) -> String {
    Url::from_file_path(path)
        .map(|u| u.to_string())
        .unwrap_or_else(|_| format!("file://{}", path))
}

pub(super) fn bounded_string(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value[..boundary].to_owned()
}

pub(super) fn uri_to_bounded_path(uri: &str) -> Option<String> {
    let url = Url::parse(uri).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    let path = url.to_file_path().ok()?.to_string_lossy().into_owned();
    (path.len() <= MAX_LSP_PATH_BYTES).then_some(path)
}

pub(super) fn parse_lsp_range(value: &Value) -> Option<LspRange> {
    serde_json::from_value(value.clone()).ok()
}

fn parse_location_like(v: &Value) -> Option<LspLocation> {
    if let Ok(loc) = serde_json::from_value::<LspLocation>(v.clone()) {
        return Some(loc);
    }
    let uri = v
        .get("targetUri")
        .or_else(|| v.get("uri"))?
        .as_str()?
        .to_string();
    let range_value = v
        .get("targetSelectionRange")
        .or_else(|| v.get("targetRange"))
        .or_else(|| v.get("range"))?;
    let range = serde_json::from_value::<LspRange>(range_value.clone()).ok()?;
    Some(LspLocation { uri, range })
}

pub(super) fn parse_locations(result: &Value) -> Vec<LspLocation> {
    if result.is_null() {
        return vec![];
    }
    if let Some(arr) = result.as_array() {
        return arr.iter().filter_map(parse_location_like).collect();
    }
    parse_location_like(result).into_iter().collect()
}

fn parse_symbol_kind(value: &Value) -> Option<u32> {
    u32::try_from(value.as_u64()?).ok()
}

fn parse_document_symbol(
    value: &Value,
    depth: usize,
    remaining: &mut usize,
) -> Option<LspDocumentSymbol> {
    if depth >= MAX_DOCUMENT_SYMBOL_DEPTH || *remaining == 0 {
        return None;
    }

    let name = bounded_string(value.get("name")?.as_str()?, MAX_SYMBOL_NAME_BYTES);
    let kind = parse_symbol_kind(value.get("kind")?)?;
    let (detail, range, selection_range, child_values) =
        if let Some(location) = value.get("location") {
            // SymbolInformation[] is the legacy flat response. containerName is
            // the only useful detail; the location range is also its selection.
            let range = parse_lsp_range(location.get("range")?)?;
            let detail = value
                .get("containerName")
                .and_then(Value::as_str)
                .map(|detail| bounded_string(detail, MAX_SYMBOL_DETAIL_BYTES));
            (detail, range.clone(), range, None)
        } else {
            let range = parse_lsp_range(value.get("range")?)?;
            let selection_range = value
                .get("selectionRange")
                .and_then(parse_lsp_range)
                .unwrap_or_else(|| range.clone());
            let detail = value
                .get("detail")
                .and_then(Value::as_str)
                .map(|detail| bounded_string(detail, MAX_SYMBOL_DETAIL_BYTES));
            (
                detail,
                range,
                selection_range,
                value.get("children").and_then(Value::as_array),
            )
        };

    *remaining -= 1;
    let mut children = Vec::new();
    if let Some(child_values) = child_values {
        for child in child_values {
            if *remaining == 0 {
                break;
            }
            if let Some(child) = parse_document_symbol(child, depth + 1, remaining) {
                children.push(child);
            }
        }
    }
    Some(LspDocumentSymbol {
        name,
        detail,
        kind,
        range,
        selection_range,
        children,
    })
}

pub(super) fn parse_document_symbols(result: &Value) -> Vec<LspDocumentSymbol> {
    let Some(values) = result.as_array() else {
        return Vec::new();
    };
    let mut remaining = MAX_DOCUMENT_SYMBOLS;
    let mut symbols = Vec::new();
    for value in values {
        if remaining == 0 {
            break;
        }
        if let Some(symbol) = parse_document_symbol(value, 0, &mut remaining) {
            symbols.push(symbol);
        }
    }
    symbols
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_range() -> Value {
        json!({
            "start": { "line": 1, "character": 2 },
            "end": { "line": 3, "character": 4 }
        })
    }

    fn hierarchical_symbol(name: &str, children: Vec<Value>) -> Value {
        json!({
            "name": name,
            "detail": "d".repeat(MAX_SYMBOL_DETAIL_BYTES + 20),
            "kind": 12,
            "range": test_range(),
            "selectionRange": test_range(),
            "children": children
        })
    }

    fn symbol_depth(symbol: &LspDocumentSymbol) -> usize {
        1 + symbol.children.iter().map(symbol_depth).max().unwrap_or(0)
    }

    #[test]
    fn document_symbols_support_both_shapes_and_bound_depth_strings_and_count() {
        let mut nested = hierarchical_symbol("leaf", Vec::new());
        for _ in 0..MAX_DOCUMENT_SYMBOL_DEPTH + 5 {
            nested = hierarchical_symbol(&"é".repeat(MAX_SYMBOL_NAME_BYTES), vec![nested]);
        }
        let symbols = parse_document_symbols(&json!([nested]));
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbol_depth(&symbols[0]), MAX_DOCUMENT_SYMBOL_DEPTH);
        assert!(symbols[0].name.len() <= MAX_SYMBOL_NAME_BYTES);
        assert!(symbols[0]
            .detail
            .as_ref()
            .is_some_and(|detail| detail.len() == MAX_SYMBOL_DETAIL_BYTES));

        let flat = parse_document_symbols(&json!([{
            "name": "legacy",
            "kind": 5,
            "containerName": "Container",
            "location": {
                "uri": "file:///tmp/main.rs",
                "range": test_range()
            }
        }]));
        assert_eq!(flat.len(), 1);
        assert_eq!(flat[0].detail.as_deref(), Some("Container"));
        assert_eq!(flat[0].range, flat[0].selection_range);
        assert!(flat[0].children.is_empty());

        let many = (0..MAX_DOCUMENT_SYMBOLS + 10)
            .map(|index| hierarchical_symbol(&format!("symbol-{index}"), Vec::new()))
            .collect::<Vec<_>>();
        assert_eq!(
            parse_document_symbols(&Value::Array(many)).len(),
            MAX_DOCUMENT_SYMBOLS
        );
    }
}
