//! A small record the core keeps in one file of the data folder, written
//! whole each time it changes, so it outlives a crash and a restart.

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::Serialize;

pub(crate) struct Kept {
    path: Option<PathBuf>,
}

impl Kept {
    /// Without a data folder nothing is kept.
    pub(crate) fn new(data_dir: Option<&Path>, file: &str) -> Self {
        Self {
            path: data_dir.map(|dir| dir.join(file)),
        }
    }

    /// What the file holds, or the default when there is none yet. A file
    /// that cannot be read is moved aside first, so the next save does not
    /// overwrite what it held.
    pub(crate) fn load<T: DeserializeOwned + Default>(&self) -> T {
        let Some(path) = &self.path else {
            return T::default();
        };
        let read = match std::fs::read(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return T::default(),
            read => read.map_err(|error| error.to_string()),
        };
        read.and_then(|bytes| serde_json::from_slice(&bytes).map_err(|error| error.to_string()))
            .unwrap_or_else(|error| {
                self.set_aside(&error);
                T::default()
            })
    }

    /// Moves a file that cannot be used to `<file>.unreadable-<when>`.
    pub(crate) fn set_aside(&self, why: &str) {
        let Some(path) = &self.path else {
            return;
        };
        let mut aside = path.clone().into_os_string();
        aside.push(format!(
            ".unreadable-{}",
            sikemux_process::clock::epoch_ms()
        ));
        let moved = std::fs::rename(path, &aside);
        eprintln!(
            "sikemux core: {} could not be read ({why}); {}",
            path.display(),
            match moved {
                Ok(()) => format!("it was kept as {}", Path::new(&aside).display()),
                Err(error) => format!("it could not be moved aside either: {error}"),
            }
        );
    }

    pub(crate) fn save<T: Serialize>(&self, value: &T) {
        let Some(path) = &self.path else {
            return;
        };
        let written = serde_json::to_vec(value)
            .map_err(std::io::Error::from)
            .and_then(|bytes| sikemux_process::files::write_atomic(path, &bytes, true));
        if let Err(error) = written {
            eprintln!("sikemux core: {} was not saved: {error}", path.display());
        }
    }
}
