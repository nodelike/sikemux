//! A simulator's screen off macOS, where there are no simulators to stream.

use std::sync::{Arc, Mutex};

pub type Picture = Arc<Mutex<Option<Frame>>>;

pub type Turn = Arc<std::sync::atomic::AtomicI32>;

pub enum Frame {}

pub struct Decoder;

impl Decoder {
    pub fn new(_: Picture, _: Turn) -> Self {
        Self
    }

    pub fn decode(&mut self, _: &[u8]) -> Result<(), String> {
        Err("the simulator's screen needs macOS".into())
    }
}
