//! Windows tokens and process starts that keep the person's other programs out of a High core:
//! everything a High core starts runs on a Medium copy of its token. Nothing here exists on
//! other systems.

#[cfg(windows)]
mod child;
#[cfg(windows)]
mod descriptor;
#[cfg(windows)]
mod spawn;
#[cfg(windows)]
mod token;

#[cfg(windows)]
pub use child::{child, Child};
#[cfg(windows)]
pub use descriptor::Descriptor;
#[cfg(windows)]
pub use spawn::{person_environment, spawn, Spawned, Start};
#[cfg(windows)]
pub use token::{Integrity, Token};
