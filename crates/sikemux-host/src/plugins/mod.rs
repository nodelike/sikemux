//! The built-in plugins, which run only in the core.

mod builtin;
mod host;
mod in_core;
#[cfg(feature = "stand-in-sign-ins")]
mod stand_in;

pub use in_core::provide_to_core;
