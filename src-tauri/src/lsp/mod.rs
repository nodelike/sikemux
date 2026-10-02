// Minimal LSP client foundation: spawn a language server per (project, lang),
// frame JSON-RPC over stdio (Content-Length headers), correlate request/
// response pairs.

mod commands;
mod diagnostics;
mod discovery;
mod documents;
mod limits;
mod process;
mod protocol;
mod registry;
mod server;
mod transport;
mod types;

pub use commands::*;
pub use registry::{document_counts, drain_all, server_count};

use crate::error::AppError;

fn lsp<E: std::fmt::Display>(e: E) -> AppError {
    AppError::Lsp(e.to_string())
}
