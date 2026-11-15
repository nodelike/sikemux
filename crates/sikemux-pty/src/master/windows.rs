use std::io::{Read as _, Write};
use std::sync::{Arc, Mutex};

use portable_pty::{MasterPty, PtySize};
use tokio::sync::{mpsc, oneshot};
use windows_sys::Win32::Foundation::{CloseHandle, WAIT_FAILED};
use windows_sys::Win32::System::Threading::{
    OpenProcess, WaitForSingleObject, INFINITE, PROCESS_SYNCHRONIZE,
};

use super::Read;
use crate::process::Terminal;

const CHUNK: usize = 16 * 1024;
const CHUNKS_AHEAD: usize = 8;

type Typed = (Vec<u8>, oneshot::Sender<std::io::Result<()>>);

/// A pseudoconsole's pipes block, so a thread reads its output and another writes the typing.
/// Its output only ends once the pseudoconsole closes, so it closes when the program exits.
pub struct Master {
    console: Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>,
    output: tokio::sync::Mutex<Output>,
    typing: std::sync::mpsc::Sender<Typed>,
}

struct Output {
    chunks: mpsc::Receiver<Vec<u8>>,
    left: Vec<u8>,
    taken: usize,
}

impl Master {
    pub fn open(master: Box<dyn MasterPty + Send>, program: Option<u32>) -> std::io::Result<Self> {
        let mut reader = master.try_clone_reader().map_err(std::io::Error::other)?;
        let mut writer = master.take_writer().map_err(std::io::Error::other)?;
        let console = Arc::new(Mutex::new(Some(master)));

        let (sent, chunks) = mpsc::channel(CHUNKS_AHEAD);
        std::thread::Builder::new()
            .name("sikemux-pty-read".into())
            .spawn(move || {
                let mut chunk = vec![0u8; CHUNK];
                while let Ok(read @ 1..) = reader.read(&mut chunk) {
                    if sent.blocking_send(chunk[..read].to_vec()).is_err() {
                        return;
                    }
                }
            })?;

        let (typing, typed) = std::sync::mpsc::channel::<Typed>();
        std::thread::Builder::new()
            .name("sikemux-pty-write".into())
            .spawn(move || {
                while let Ok((bytes, done)) = typed.recv() {
                    let _ = done.send(writer.write_all(&bytes).and_then(|()| writer.flush()));
                }
            })?;

        if let Some(program) = program {
            let console = console.clone();
            std::thread::Builder::new()
                .name("sikemux-pty-exit".into())
                .spawn(move || {
                    wait_for_exit(program);
                    console
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .take();
                })?;
        }

        Ok(Self {
            console,
            output: tokio::sync::Mutex::new(Output {
                chunks,
                left: Vec::new(),
                taken: 0,
            }),
            typing,
        })
    }

    pub fn terminal(&self) -> Terminal {
        Terminal::pseudoconsole()
    }

    /// The next output there is; cancelling it loses none.
    pub async fn read(&self, buffer: &mut [u8]) -> Read {
        let mut output = self.output.lock().await;
        if output.taken == output.left.len() {
            let Some(chunk) = output.chunks.recv().await else {
                return Read::Eof;
            };
            output.left = chunk;
            output.taken = 0;
        }
        let Output { left, taken, .. } = &mut *output;
        let waiting = &left[*taken..];
        let read = waiting.len().min(buffer.len());
        buffer[..read].copy_from_slice(&waiting[..read]);
        *taken += read;
        Read::Bytes(read)
    }

    pub async fn write_all(&self, data: &[u8]) -> std::io::Result<()> {
        let (done, written) = oneshot::channel();
        self.typing
            .send((data.to_vec(), done))
            .map_err(|_| std::io::Error::other("pty closed"))?;
        written
            .await
            .unwrap_or_else(|_| Err(std::io::Error::other("pty closed")))
    }

    pub fn resize(&self, rows: u16, cols: u16) -> std::io::Result<()> {
        let console = self
            .console
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(console) = console.as_ref() else {
            return Ok(());
        };
        console
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(std::io::Error::other)
    }
}

fn wait_for_exit(program: u32) {
    // SAFETY: the handle is checked, waited on and closed here, and nothing else uses it.
    unsafe {
        let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, program);
        if process.is_null() {
            return;
        }
        if WaitForSingleObject(process, INFINITE) == WAIT_FAILED {
            CloseHandle(process);
            return;
        }
        CloseHandle(process);
    }
}
