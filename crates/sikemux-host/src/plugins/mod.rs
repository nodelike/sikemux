//! The built-in plugins, which run only in the core.

mod builtin;
mod host;
mod in_core;

pub use in_core::provide_to_core;
