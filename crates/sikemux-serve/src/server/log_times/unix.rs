use std::os::fd::RawFd;

pub type Handle = RawFd;

pub const CHUNK: usize = 16 * 1024;

pub enum Read {
    Bytes(usize),
    Empty,
    Closed,
}

/// Keeps the log file stderr names and puts a pipe in its place, answering both.
pub fn take_stderr() -> Option<(RawFd, RawFd)> {
    // SAFETY: fstat fills `stat` for descriptor 2; the calls after it only duplicate,
    // create and close descriptors this function owns, and checked each answer.
    unsafe {
        let mut stat = std::mem::zeroed::<libc::stat>();
        if libc::fstat(libc::STDERR_FILENO, &mut stat) != 0
            || stat.st_mode & libc::S_IFMT != libc::S_IFREG
        {
            return None;
        }
        let log = libc::fcntl(libc::STDERR_FILENO, libc::F_DUPFD_CLOEXEC, 3);
        if log < 0 {
            return None;
        }
        let mut ends = [0; 2];
        if libc::pipe(ends.as_mut_ptr()) != 0 {
            libc::close(log);
            return None;
        }
        let [read, write] = ends;
        let status = libc::fcntl(read, libc::F_GETFL);
        libc::fcntl(read, libc::F_SETFL, status | libc::O_NONBLOCK);
        let flags = libc::fcntl(read, libc::F_GETFD);
        libc::fcntl(read, libc::F_SETFD, flags | libc::FD_CLOEXEC);
        if libc::dup2(write, libc::STDERR_FILENO) < 0 {
            libc::close(read);
            libc::close(write);
            libc::close(log);
            return None;
        }
        libc::close(write);
        Some((log, read))
    }
}

pub fn give_back(log: RawFd, pipe: RawFd) {
    // SAFETY: both descriptors are this module's own; dup2 only repoints descriptor 2.
    unsafe {
        libc::dup2(log, libc::STDERR_FILENO);
        libc::close(pipe);
        libc::close(log);
    }
}

/// Nothing is left to let go: [`give_back`] closed the pipe.
pub fn release(_pipe: RawFd) {}

pub fn copy_of(log: Option<RawFd>) -> Option<std::fs::File> {
    let fd = log.unwrap_or(libc::STDERR_FILENO);
    // SAFETY: duplicating a descriptor this process holds; the copy is owned by the File.
    unsafe {
        let copy = libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3);
        (copy >= 0).then(|| std::os::fd::FromRawFd::from_raw_fd(copy))
    }
}

/// Waits until `pipe` has something to read; nothing is read here.
pub fn wait(pipe: RawFd) -> Result<Vec<u8>, ()> {
    loop {
        let mut wait = libc::pollfd {
            fd: pipe,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `wait` is one valid pollfd.
        let polled = unsafe { libc::poll(&mut wait, 1, -1) };
        if polled < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(());
        }
        if wait.revents & libc::POLLNVAL != 0 {
            return Err(());
        }
        return Ok(Vec::new());
    }
}

pub fn read(pipe: RawFd, buffer: &mut [u8]) -> Read {
    // SAFETY: `buffer` has room for as many bytes as asked for.
    let read = unsafe { libc::read(pipe, buffer.as_mut_ptr().cast(), buffer.len()) };
    if read > 0 {
        return Read::Bytes(read as usize);
    }
    if read == 0 {
        return Read::Closed;
    }
    match std::io::Error::last_os_error().kind() {
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted => Read::Empty,
        _ => Read::Closed,
    }
}

pub fn write_all(fd: RawFd, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        // SAFETY: `bytes` is valid for its length.
        let written = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if written <= 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
        bytes = bytes.get(written as usize..).unwrap_or_default();
    }
}
