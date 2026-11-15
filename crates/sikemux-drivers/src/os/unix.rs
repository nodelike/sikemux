use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;

pub fn read_exact_at(file: &File, buffer: &mut [u8], offset: u64) -> io::Result<()> {
    file.read_exact_at(buffer, offset)
}
