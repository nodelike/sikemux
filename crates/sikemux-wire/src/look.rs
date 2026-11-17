//! A browser on the person's account reading a host's look. It opens one stream over
//! [`LOOK_ALPN`] and sends its look-only join ticket as a control frame; the host answers with
//! one control frame, a `LookReply` (`sikemux-view`), and closes. The host never pairs the
//! browser, so the ticket gets it this answer and nothing more.

pub const LOOK_ALPN: &[u8] = b"sikemux/look/1";

/// The answer carries the backdrop's picture, a JPEG no longer than 1280 px.
pub const MAX_LOOK_BYTES: usize = 4 * 1024 * 1024;
