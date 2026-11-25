use std::sync::mpsc;

use super::log::{CallLog, Entry};

enum LogOp {
    Append {
        project: String,
        entry: Box<Entry>,
        sure: bool,
        still_open: Vec<Entry>,
        done: Option<mpsc::SyncSender<Result<(), String>>>,
    },
    Flush(mpsc::SyncSender<()>),
}

pub(super) struct LogWriter {
    tx: Option<mpsc::Sender<LogOp>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl LogWriter {
    pub(super) fn start(log: CallLog) -> Self {
        if !log.has_folder() {
            return Self {
                tx: None,
                worker: None,
            };
        }
        let (tx, rx) = mpsc::channel::<LogOp>();
        let worker = std::thread::Builder::new()
            .name("sikemux-call-log".to_owned())
            .spawn(move || {
                while let Ok(op) = rx.recv() {
                    match op {
                        LogOp::Append {
                            project,
                            entry,
                            sure,
                            still_open,
                            done,
                        } => {
                            let res = log.append(&project, &entry, sure, || still_open);
                            if let Some(done) = done {
                                let _ = done.send(res);
                            } else if let Err(error) = res {
                                eprintln!("sikemux core: a call was not written down: {error}");
                            }
                        }
                        LogOp::Flush(done) => {
                            let _ = done.send(());
                        }
                    }
                }
            })
            .ok();
        match worker {
            Some(handle) => Self {
                tx: Some(tx),
                worker: Some(handle),
            },
            None => Self {
                tx: None,
                worker: None,
            },
        }
    }

    pub(super) fn enqueue(&self, project: String, entry: Entry, still_open: Vec<Entry>) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(LogOp::Append {
                project,
                entry: Box::new(entry),
                sure: false,
                still_open,
                done: None,
            });
        }
    }

    pub(super) fn append_sure(
        &self,
        log: &CallLog,
        project: String,
        entry: Entry,
        still_open: Vec<Entry>,
    ) -> Result<(), String> {
        let Some(tx) = &self.tx else {
            return log.append(&project, &entry, true, || still_open);
        };
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        tx.send(LogOp::Append {
            project,
            entry: Box::new(entry),
            sure: true,
            still_open,
            done: Some(done_tx),
        })
        .map_err(|_| "call log writer stopped".to_owned())?;
        done_rx
            .recv()
            .map_err(|_| "call log writer stopped".to_owned())?
    }

    pub(super) fn flush(&self) {
        let Some(tx) = &self.tx else {
            return;
        };
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        if tx.send(LogOp::Flush(done_tx)).is_ok() {
            let _ = done_rx.recv();
        }
    }
}

impl Drop for LogWriter {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
