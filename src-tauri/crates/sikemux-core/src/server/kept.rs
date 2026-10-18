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
                eprintln!(
                    "sikemux core: {} could not be read: {error}",
                    path.display()
                );
                T::default()
            })
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
