//! What a phone build still in the stores reads differently from this core's JSON, put right on its
//! connection alone.
//! phone-compat: 0.2.0 dotSize paneDotSize customThemes paneShader cloudBrowser cloudBrowserShortcut running next assertions blockers

use std::borrow::Cow;

use serde_json::Value;
use sikemux_wire::protocol::{encode_frame, FrameKind};

/// Phones that agreed a protocol below 16 read dot sizes as whole numbers.
pub fn needs(version: u32) -> bool {
    version < 16
}

pub fn fit(older: bool, frame: &[u8]) -> Cow<'_, [u8]> {
    let Some((&kind, payload)) = frame.get(4..).and_then(<[u8]>::split_first) else {
        return Cow::Borrowed(frame);
    };
    if !older || kind != FrameKind::Control as u8 || !mentions_any(payload) {
        return Cow::Borrowed(frame);
    }
    let Ok(mut message) = serde_json::from_slice::<Value>(payload) else {
        return Cow::Borrowed(frame);
    };
    put_right(&mut message);
    match serde_json::to_vec(&message) {
        Ok(payload) => Cow::Owned(encode_frame(FrameKind::Control, &[&payload])),
        Err(_) => Cow::Borrowed(frame),
    }
}

fn mentions_any(payload: &[u8]) -> bool {
    [&b"otSize\""[..], b"\"cards\"", b"\"downloadedBytes\""]
        .iter()
        .any(|key| payload.windows(key.len()).any(|window| window == *key))
}

fn put_right(value: &mut Value) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(put_right),
        Value::Object(object) => {
            for key in ["dotSize", "paneDotSize"] {
                if let Some(size) = object.get(key).and_then(Value::as_f64) {
                    object.insert(key.into(), Value::from(size.round() as i64));
                }
            }
            if object.contains_key("paneDotSize") {
                object
                    .entry("customThemes")
                    .or_insert(Value::Array(Vec::new()));
                object.entry("paneShader").or_insert(Value::Bool(false));
                for key in ["cloudBrowser", "cloudBrowserShortcut"] {
                    object.entry(key).or_insert(Value::from(""));
                }
            }
            if object.contains_key("cards") && object.contains_key("settings") {
                object.entry("running").or_insert(Value::from(0));
            }
            if object.contains_key("downloadedBytes") {
                object.entry("blockers").or_insert(Value::Array(Vec::new()));
            }
            if object.contains_key("acceptance") && object.contains_key("column") {
                object.entry("next").or_insert(Value::Array(Vec::new()));
                object
                    .entry("assertions")
                    .or_insert(Value::Array(Vec::new()));
            }
            object.values_mut().for_each(put_right);
        }
        _ => {}
    }
}
