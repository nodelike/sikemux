//! Where on a piece of a slice's evidence a change the person asked for is pinned.

use super::*;

/// `spot` as it can sit on a piece of evidence that is `proof`.
pub fn spot_on(proof: &Proof, spot: Spot) -> Result<Spot, String> {
    match (proof, &spot) {
        (
            Proof::Picture {
                seconds: length, ..
            },
            Spot::Point { x, y, seconds },
        ) => {
            let inside = |at: f32| (0.0..=1.0).contains(&at);
            if !inside(*x) || !inside(*y) {
                return Err("a point's x and y are fractions of the picture, from 0 to 1".into());
            }
            match (length, seconds) {
                (None, Some(_)) => Err("a screenshot has no frames to pin to".into()),
                (Some(length), Some(at)) if !(0.0..=*length).contains(at) => {
                    Err(format!("the recording is {length:.1} seconds long"))
                }
                _ => Ok(spot),
            }
        }
        (
            Proof::Picture {
                seconds: Some(length),
                ..
            },
            Spot::Moment { seconds },
        ) => match (0.0..=*length).contains(seconds) {
            true => Ok(spot),
            false => Err(format!("the recording is {length:.1} seconds long")),
        },
        (Proof::Diff { .. }, Spot::Line { file, line }) if !file.is_empty() && *line > 0 => {
            Ok(spot)
        }
        (Proof::Diff { .. }, Spot::Line { .. }) => Err("a line needs its file and number".into()),
        _ => Err(format!("a {} takes no such spot", proof_words(proof))),
    }
}

/// Where on its evidence a change is pinned, to follow the evidence's name.
pub fn spot_words(spot: &Spot) -> String {
    match spot {
        Spot::Point {
            seconds: Some(at), ..
        }
        | Spot::Moment { seconds: at } => format!(" at {at:.1}s"),
        Spot::Point { x, y, .. } => {
            format!(" at {:.0}% across and {:.0}% down", x * 100.0, y * 100.0)
        }
        Spot::Line { file, line } => format!(" at {file}:{line}"),
    }
}
