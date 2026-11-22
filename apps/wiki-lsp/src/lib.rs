//! Wiki LSP MVP.
//!
//! A deliberately small stdio LSP server for Neovim 0.10+. It reuses
//! `harmion-wiki-core` for syntax and talks to the read-only `wiki-server`
//! Unix-socket JSON-RPC for indexing and link resolution.
//!
//! Guarantees:
//! - no save/write path and no credentials/tokens;
//! - local fence diagnostics keep working when the server is unavailable;
//! - definition/references return an empty result instead of failing when the
//!   server is unavailable.

pub mod completion;
pub mod diagnostics;
pub mod engine;
pub mod framing;
pub mod hover;
pub mod protocol;
pub mod providers;
pub mod server;
pub mod text;
pub mod uri;

pub use engine::LspServer;
pub use server::DEFAULT_SOCKET;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
