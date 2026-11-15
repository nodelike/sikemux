use std::io::{Read, Write};
use std::sync::mpsc;

use portable_pty::MasterPty;

/// What a pseudoconsole prints, chunk by chunk, until it closes. A pseudoconsole asks
/// where its cursor is before it runs anything, so this answers as a terminal would.
pub(crate) fn pseudoconsole_output(master: &dyn MasterPty) -> mpsc::Receiver<Vec<u8>> {
    let mut reader = master.try_clone_reader().expect("clone reader");
    let mut writer = master.take_writer().expect("take writer");
    let (chunks, arrivals) = mpsc::channel();
    std::thread::spawn(move || {
        let mut chunk = [0_u8; 4096];
        while let Ok(read @ 1..) = reader.read(&mut chunk) {
            let chunk = &chunk[..read];
            if chunk.windows(4).any(|query| query == b"\x1b[6n") {
                let _ = writer.write_all(b"\x1b[1;1R");
            }
            if chunks.send(chunk.to_vec()).is_err() {
                return;
            }
        }
    });
    arrivals
}
