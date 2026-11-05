//! The screenshots a slice keeps as evidence: when one is wanted, and its files.

use super::*;

/// The viewport presets, a phone's and a tablet's, a screenshot of a slice's page is also
/// taken at, so its evidence shows the page at those widths too.
pub(crate) const SCREENSHOT_WIDTHS: [&str; 2] = ["mobile", "tablet"];

/// Whether a screenshot `agent` takes now is kept as evidence: the card it works is a
/// slice with assertions a screenshot shows.
pub(crate) fn wants_screenshot(core: &Core, agent: &str) -> bool {
    let state = core.boards.state.locked();
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
pub(crate) fn screenshot_taken(core: &Arc<Core>, agent: &str, shots: &[Value]) {
    let decoded = decode_shots(shots);
    if decoded.is_empty() {
        return;
    }
    let target = {
        let state = core.boards.state.locked();
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
pub(crate) type Shot = (String, Option<u32>);

/// The pictures in the browser's answers to screenshots, each with the width
/// it was taken at.
pub(crate) fn decode_shots(shots: &[Value]) -> Vec<(Vec<u8>, Option<u32>)> {
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
pub(crate) fn keep_shots(
    core: &Arc<Core>,
    project: &str,
    id: u64,
    decoded: &[(Vec<u8>, Option<u32>)],
    collect: impl FnOnce(&mut Slice, &[Shot]),
) {
    let paths = {
        let mut state = core.boards.state.locked();
        let Some(paths) = (decoded.iter())
            .map(|(_, width)| core.boards.evidence_file(project, id, *width))
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
    if let Some(board) = core.boards.state.locked().boards.get_mut(project) {
        board.writing.retain(|writing| !paths.contains(writing));
    }
}
