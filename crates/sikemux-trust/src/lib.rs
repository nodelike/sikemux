//! Windows tokens and process starts that keep the person's other programs out of a High core:
//! everything a High core starts runs on a Medium copy of its token. On an install with the
//! Sikemux service, the service starts each person's core from their own lowered token. Nothing
//! here exists on other systems.

#[cfg(windows)]
mod child;
#[cfg(windows)]
mod descriptor;
#[cfg(windows)]
mod library;
#[cfg(windows)]
mod pipe;
#[cfg(windows)]
pub mod service;
#[cfg(windows)]
mod spawn;
#[cfg(windows)]
mod token;
#[cfg(windows)]
mod update;
#[cfg(windows)]
mod window;

#[cfg(windows)]
pub use child::{child, Child};
#[cfg(windows)]
pub use descriptor::Descriptor;
#[cfg(windows)]
pub use library::{loaded_libraries, pin_library_search};
#[cfg(windows)]
pub use spawn::{person_environment, spawn, Spawned, Start};
#[cfg(windows)]
pub use token::{Integrity, Token};
#[cfg(windows)]
pub use window::confirm;
