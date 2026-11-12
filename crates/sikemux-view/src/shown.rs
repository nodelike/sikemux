//! What finished work shows: the page its change touched, taken by the core at a phone's, a
//! tablet's and a desktop's width beside the last pictures of the same page, and in a lead's chat
//! a row for each worker's finished work. The pictures are shown, never proven.

use serde::{Deserialize, Serialize};

use crate::turn::TurnDone;

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
    /// The JPEG on the host, which a client reads as it reads a chat's files.
    pub path: String,
    pub at: u64,
}

/// A worker's finished work, as its lead's chat shows it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultRow {
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// What it worked on: its chat's title.
    pub title: String,
    /// When its turn ended.
    pub at: u64,
    /// Where its work is and how its checks stand, as that worker's own turn says, with
    /// `tryIt` among the next steps when its page can be served from its checkout.
    pub done: TurnDone,
    /// Its commits are on the project's branch, so `takeStep` with `undo` can take them out.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub undo: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<PageShown>,
}
