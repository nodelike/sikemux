use std::path::PathBuf;

use crate::local::Connection;

pub struct Peer;

impl Peer {
    pub fn pid(&self) -> u32 {
        0
    }

    pub fn uid(&self) -> u32 {
        u32::MAX
    }

    pub fn program(&self) -> Option<PathBuf> {
        None
    }
}

pub fn peer(_connection: &Connection) -> Option<Peer> {
    None
}
