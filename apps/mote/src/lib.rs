//! Shared parts of Mote that do not parse a command line: the credential store,
//! the local outbox, and the errors they report.

pub mod auth;
pub mod spool;

use std::{
    io,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

/// The node's 1 MiB record limit, with room for the envelope.
pub const MAX_CAPTURE_BYTES: usize = 1024 * 1024 - 64 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Client(#[from] harmion_client::Error),
    #[error(transparent)]
    Capture(#[from] mote_core::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Message(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// `$MOTE_STATE_DIR`, else `$XDG_STATE_HOME/mote`, else `~/.local/state/mote`.
pub fn state_dir() -> Result<PathBuf> {
    if let Some(value) = std::env::var_os("MOTE_STATE_DIR") {
        let value = value
            .into_string()
            .map_err(|_| Error::Message("MOTE_STATE_DIR is not valid UTF-8".into()))?;
        if value.trim().is_empty() {
            return Err(Error::Message("MOTE_STATE_DIR is empty".into()));
        }
        return Ok(PathBuf::from(value));
    }
    if let Some(value) = std::env::var_os("XDG_STATE_HOME") {
        let value = value
            .into_string()
            .map_err(|_| Error::Message("XDG_STATE_HOME is not valid UTF-8".into()))?;
        if !value.trim().is_empty() {
            return Ok(PathBuf::from(value).join("mote"));
        }
    }
    let home = std::env::var_os("HOME")
        .ok_or_else(|| Error::Message("HOME is not set; set MOTE_STATE_DIR".into()))?;
    Ok(PathBuf::from(home).join(".local/state/mote"))
}

pub fn now_micros() -> Result<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::Message("system clock is before the Unix epoch".into()))
        .and_then(|duration| {
            i64::try_from(duration.as_micros())
                .map_err(|_| Error::Message("system clock is out of range".into()))
        })
}
