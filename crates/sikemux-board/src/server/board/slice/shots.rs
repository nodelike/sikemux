//! The screenshots a slice keeps as evidence: when one is wanted, and its files.

use crate::server::board_state::BoardParts;
use serde_json::json;

use super::*;

/// The viewport presets, a phone's and a tablet's, a screenshot of a slice's page is also
/// taken at, so its evidence shows the page at those widths too.
pub const SCREENSHOT_WIDTHS: [&str; 2] = ["mobile", "tablet"];

/// Whether a screenshot `agent` takes now is kept as evidence: the card it works is a
/// slice with assertions a screenshot shows.
pub fn wants_screenshot(core: &Core, agent: &str) -> bool {
    let state = core.boards().state.locked();
    state.boards.values().any(|board| {
        working_slice(board, agent)
            .and_then(|id| board.card(id).ok()?.slice.as_ref())
            .is_some_and(|slice| !slice.asserting(Collect::Screenshot).is_empty())
    })
}

/// The agent took a screenshot, and the core took the same page beside it at each
/// width it is held at: when the card the agent works is a slice with assertions a
/// screenshot shows, the core keeps them all as their evidence, each in a file of its
/// own, and moves the slice on. `shots` are the browser's answers, the agent's first.
/// The board's writer removes a file once no evidence names it.
pub fn screenshot_taken(core: &Arc<Core>, agent: &str, shots: &[Value]) {
    let decoded = decode_shots(shots);
    if decoded.is_empty() {
        return;
    }
    let target = {
        let state = core.boards().state.locked();
        state.boards.iter().find_map(|(project, board)| {
            let id = working_slice(board, agent)?;
            let slice = board.card(id).ok()?.slice.as_ref()?;
            (!slice.asserting(Collect::Screenshot).is_empty()).then(|| (project.clone(), id))
        })
    };
    if let Some((project, id)) = target {
        keep_shots(core, &project, id, &decoded, |slice, kept| {
            slice.screenshots(agent, kept)
        });
    }
}

/// A screenshot kept as evidence: its file, and the width the page was laid out at.
pub type Shot = (String, Option<u32>);

/// The pictures in the browser's answers to screenshots, each with the width
/// it was taken at.
pub fn decode_shots(shots: &[Value]) -> Vec<(Vec<u8>, Option<u32>)> {
    use base64::Engine;
    shots
        .iter()
        .filter_map(|shot| {
            let bytes = (shot.get("data").and_then(Value::as_str))
                .and_then(|data| base64::engine::general_purpose::STANDARD.decode(data).ok())?;
            let width = shot
                .pointer("/viewport/width")
                .and_then(Value::as_u64)
                .and_then(|width| u32::try_from(width).ok());
            Some((bytes, width))
        })
        .collect()
}

/// Writes `decoded` each to a file of its own and has the slice on `id`
/// keep them with `collect`, then moves the slice on. The board's writer
/// removes a file once no evidence names it.
pub fn keep_shots(
    core: &Arc<Core>,
    project: &str,
    id: u64,
    decoded: &[(Vec<u8>, Option<u32>)],
    collect: impl FnOnce(&mut Slice, &[Shot]),
) {
    let paths = {
        let mut state = core.boards().state.locked();
        let Some(paths) = (decoded.iter())
            .map(|(_, width)| core.boards().evidence_file(project, id, *width))
            .collect::<Option<Vec<_>>>()
        else {
            return;
        };
        if let Some(board) = state.boards.get_mut(project) {
            board.writing.extend(paths.iter().cloned());
        }
        paths
    };
    let mut kept = Vec::new();
    for ((bytes, width), path) in decoded.iter().zip(&paths) {
        let written = (path.parent())
            .map_or(Ok(()), std::fs::create_dir_all)
            .map_err(|error| error.to_string())
            .and_then(|()| {
                sikemux_process::files::write_atomic(path, bytes, true)
                    .map_err(|error| error.to_string())
            });
        match written {
            Ok(_) => kept.push((path.to_string_lossy().into_owned(), *width)),
            Err(error) => {
                eprintln!("sikemux core: a screenshot was not kept as evidence: {error}");
            }
        }
    }
    if !kept.is_empty() {
        moved_by_evidence(core, project, id, |slice| collect(slice, &kept));
    }
    if let Some(board) = core.boards().state.locked().boards.get_mut(project) {
        board.writing.retain(|writing| !paths.contains(writing));
    }
}

/// The agent's screenshot, and when its slice keeps screenshots as evidence, the same
/// page taken beside it at each of [`SCREENSHOT_WIDTHS`], before the agent's call
/// returns so nothing the agent does next meets its tab at another width.
pub async fn slice_screenshots(
    core: &Core,
    request: &crate::cli::protocol::HarnessRequest,
    agent_id: &str,
    answer: &Value,
) -> Vec<Value> {
    let mut shots = vec![answer.clone()];
    if wants_screenshot(core, agent_id) {
        shots.extend(other_widths(core, request, agent_id).await);
    }
    shots
}

/// The page `request` took a screenshot of in `agent_id`'s tab, taken again at
/// each of [`SCREENSHOT_WIDTHS`]. Each is only a resize, so the page does
/// not reload; the tab is put back as it was held after both, whatever became
/// of them.
pub async fn other_widths(
    core: &Core,
    request: &crate::cli::protocol::HarnessRequest,
    agent_id: &str,
) -> Vec<Value> {
    let mut shots = Vec::new();
    let asked = |method: &str, params: Value, step: &str| crate::cli::protocol::HarnessRequest {
        id: format!("{}-{step}", request.id),
        project: request.project.clone(),
        agent_id: Some(agent_id.to_owned()),
        method: method.to_owned(),
        params,
    };
    let held = match (core.board_above().harness_forward)(
        core,
        asked("browser.viewport", json!({}), "held"),
    )
    .await
    {
        Ok(size) => size.get("held").cloned().unwrap_or(Value::Null),
        Err(error) => {
            eprintln!("sikemux core: the page was not taken at other widths: {error}");
            return shots;
        }
    };
    for preset in SCREENSHOT_WIDTHS {
        let mut params = json!({ "viewport": preset });
        if request.params.get("fullPage") == Some(&Value::Bool(true)) {
            params["fullPage"] = Value::Bool(true);
        }
        match (core.board_above().harness_forward)(core, asked(&request.method, params, preset))
            .await
        {
            Ok(shot) => shots.push(shot),
            Err(error) => {
                eprintln!("sikemux core: the page was not taken at the {preset} width: {error}")
            }
        }
    }
    let back = match &held {
        Value::Null => json!({ "preset": "fit" }),
        held => match held.get("preset") {
            Some(preset) => json!({ "preset": preset }),
            None => json!({ "width": held["width"], "height": held["height"] }),
        },
    };
    if let Err(error) =
        (core.board_above().harness_forward)(core, asked("browser.viewport", back, "back")).await
    {
        eprintln!("sikemux core: the tab was not put back at its own width: {error}");
    }
    shots
}
