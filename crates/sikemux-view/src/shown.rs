//! What finished work shows: the page its change touched, taken by the core at a phone's, a
//! tablet's and a desktop's width beside the last pictures of the same page. The pictures are
//! shown, never proven.

use serde::{Deserialize, Serialize};

/// The pages a turn of this chat changed, as the core took them once it finished.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnShown {
    /// When the turn began, in Unix milliseconds; its reply is the one that ended it.
    pub turn: u64,
    pub pages: Vec<PageShown>,
}

/// One page at each width, with the last pictures of it before this change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageShown {
    /// Its path on the server, such as `/invoices`.
    pub page: String,
    pub widths: Vec<WidthShown>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WidthShown {
    /// In CSS pixels: 390, 820 or 1440.
    pub width: u32,
    pub after: Picture,
    /// None the first time the page was taken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Picture>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Picture {
    /// What a client fetches the JPEG by, with `turnPicture`.
    pub id: String,
    pub at: u64,
}
