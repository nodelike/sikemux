use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use super::super::connection::FrameWriter;
use super::super::Core;
use super::{refused, reply};

const IN_PLACE: &str = "a core on Windows cannot be replaced while it runs yet";

pub(super) async fn upgrade(
    _core: &Arc<Core>,
    _binary: PathBuf,
    _force: bool,
    writer: &mut FrameWriter,
) {
    reply(writer, &refused(IN_PLACE)).await;
}

pub(super) async fn restart(_core: &Arc<Core>, writer: &mut FrameWriter) {
    reply(writer, &refused(IN_PLACE)).await;
}

pub(crate) async fn replace_with(
    _core: &Arc<Core>,
    _binary: PathBuf,
    _force: bool,
    _limit: Duration,
) -> String {
    IN_PLACE.into()
}
