use std::io::{PipeReader, PipeWriter, Read as _, Write};
use std::os::windows::io::{AsHandle, AsRawHandle, OwnedHandle, RawHandle};
use std::sync::{Arc, Mutex};

use portable_pty::{
    Child, ChildKiller, CommandBuilder, ExitStatus, MasterPty, PtyPair, PtySize, SlavePty,
};
use tokio::sync::{mpsc, oneshot};
use windows_sys::Win32::Foundation::{CloseHandle, WAIT_FAILED};
use windows_sys::Win32::System::Console::{
    ClosePseudoConsole, CreatePseudoConsole, ResizePseudoConsole, COORD, HPCON,
    PSEUDOCONSOLE_INHERIT_CURSOR,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, TerminateProcess, WaitForSingleObject, INFINITE, PROCESS_SYNCHRONIZE,
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
                    if sent
                        .blocking_send(chunk.get(..read).unwrap_or_default().to_vec())
                        .is_err()
                    {
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
        let waiting = left.get(*taken..).unwrap_or_default();
        let read = waiting.len().min(buffer.len());
        if let (Some(into), Some(from)) = (buffer.get_mut(..read), waiting.get(..read)) {
            into.copy_from_slice(from);
        }
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

// Flags Windows Terminal passes that Windows does not document; portable-pty passed them too.
const PSEUDOCONSOLE_RESIZE_QUIRK: u32 = 0x2;
const PSEUDOCONSOLE_WIN32_INPUT_MODE: u32 = 0x4;

/// A new pseudoconsole. Its program starts through the core's own command, so it gets the
/// token every child of the core starts on.
pub fn open_pty(size: PtySize) -> std::io::Result<PtyPair> {
    let (console_input, input) = std::io::pipe()?;
    let (output, console_output) = std::io::pipe()?;
    let mut console: HPCON = 0;
    // SAFETY: both pipe ends are open for the call, and the pseudoconsole keeps copies of its own.
    let result = unsafe {
        CreatePseudoConsole(
            coord(size),
            console_input.as_raw_handle(),
            console_output.as_raw_handle(),
            PSEUDOCONSOLE_INHERIT_CURSOR
                | PSEUDOCONSOLE_RESIZE_QUIRK
                | PSEUDOCONSOLE_WIN32_INPUT_MODE,
            &mut console,
        )
    };
    if result < 0 {
        return Err(std::io::Error::other(format!(
            "CreatePseudoConsole failed: HRESULT {result:#x}"
        )));
    }
    let shared = Arc::new(Pseudoconsole {
        console: Console(console),
        output,
        input: Mutex::new(Some(input)),
        size: Mutex::new(size),
    });
    Ok(PtyPair {
        slave: Box::new(ConsoleSlave(shared.clone())),
        master: Box::new(ConsoleMaster(shared)),
    })
}

fn coord(size: PtySize) -> COORD {
    COORD {
        X: size.cols as i16,
        Y: size.rows as i16,
    }
}

struct Console(HPCON);

impl Drop for Console {
    fn drop(&mut self) {
        // SAFETY: the pseudoconsole was created open and is closed only here.
        unsafe { ClosePseudoConsole(self.0) };
    }
}

struct Pseudoconsole {
    console: Console,
    output: PipeReader,
    input: Mutex<Option<PipeWriter>>,
    size: Mutex<PtySize>,
}

struct ConsoleMaster(Arc<Pseudoconsole>);

impl MasterPty for ConsoleMaster {
    fn resize(&self, size: PtySize) -> anyhow::Result<()> {
        // SAFETY: the pseudoconsole stays open while `self` holds it.
        let result = unsafe { ResizePseudoConsole(self.0.console.0, coord(size)) };
        anyhow::ensure!(
            result >= 0,
            "ResizePseudoConsole failed: HRESULT {result:#x}"
        );
        *self
            .0
            .size
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = size;
        Ok(())
    }

    fn get_size(&self) -> anyhow::Result<PtySize> {
        Ok(*self
            .0
            .size
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()))
    }

    fn try_clone_reader(&self) -> anyhow::Result<Box<dyn std::io::Read + Send>> {
        Ok(Box::new(self.0.output.try_clone()?))
    }

    fn take_writer(&self) -> anyhow::Result<Box<dyn std::io::Write + Send>> {
        let writer = self
            .0
            .input
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .ok_or_else(|| anyhow::anyhow!("pseudoconsole writer already taken"))?;
        Ok(Box::new(writer))
    }
}

struct ConsoleSlave(Arc<Pseudoconsole>);

impl SlavePty for ConsoleSlave {
    fn spawn_command(&self, built: CommandBuilder) -> anyhow::Result<Box<dyn Child + Send + Sync>> {
        let mut command = command(&built)?;
        command.pseudoconsole(self.0.console.0);
        Ok(Box::new(ConsoleChild(sikemux_process::spawn_tree(
            &mut command,
        )?)))
    }
}

/// `built` as the core's own command. The variables it set are set again, and the ones it
/// dropped from this process's are removed, on top of what the seam starts every child with.
fn command(built: &CommandBuilder) -> anyhow::Result<sikemux_process::command::Command> {
    let [program, args @ ..] = built.get_argv().as_slice() else {
        anyhow::bail!("a pseudoconsole needs a program to start");
    };
    #[allow(clippy::disallowed_methods)]
    let mut command = sikemux_process::command::Command::new(program);
    command.args(args);
    if let Some(dir) = built.get_cwd() {
        command.current_dir(dir);
    }
    for (key, _) in std::env::vars_os() {
        if built.get_env(&key).is_none() {
            command.env_remove(key);
        }
    }
    for (key, value) in built.iter_extra_env_as_str() {
        command.env(key, value);
    }
    Ok(command)
}

struct ConsoleChild(sikemux_process::command::Child);

impl std::fmt::Debug for ConsoleChild {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ConsoleChild").field(&self.0.id()).finish()
    }
}

impl Child for ConsoleChild {
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        Ok(self.0.try_wait()?.map(ExitStatus::from))
    }

    fn wait(&mut self) -> std::io::Result<ExitStatus> {
        Ok(self.0.wait()?.into())
    }

    fn process_id(&self) -> Option<u32> {
        Some(self.0.id())
    }

    fn as_raw_handle(&self) -> Option<RawHandle> {
        Some(self.0.as_raw_handle())
    }
}

impl ChildKiller for ConsoleChild {
    fn kill(&mut self) -> std::io::Result<()> {
        self.0.kill()
    }

    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(Killer(self.0.as_handle().try_clone_to_owned().ok()))
    }
}

#[derive(Debug)]
struct Killer(Option<OwnedHandle>);

impl ChildKiller for Killer {
    fn kill(&mut self) -> std::io::Result<()> {
        let Some(process) = &self.0 else {
            return Err(std::io::Error::other(
                "the program's handle could not be copied",
            ));
        };
        // SAFETY: the handle is a copy this killer owns, open until it drops.
        if unsafe { TerminateProcess(process.as_raw_handle(), 1) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(Killer(
            self.0.as_ref().and_then(|process| process.try_clone().ok()),
        ))
    }
}
