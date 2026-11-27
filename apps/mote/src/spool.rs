//! Durable outbox for short captures.
//!
//! A record is written before the save request is sent. The content and request
//! IDs are fixed in the record, so retrying after an ambiguous response is
//! idempotent as long as the node kept the request receipt.

use harmion_client::{protocol::hex, random_id};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, DirBuilder, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use crate::{Error, Result};

#[derive(Clone, Serialize, Deserialize)]
pub struct PendingCapture {
    pub version: u8,
    pub content_id: String,
    pub request_id: String,
    pub created_at_micros: i64,
    pub body: String,
    #[serde(default)]
    pub reply_to: Option<String>,
}

impl PendingCapture {
    pub fn new(body: String, reply_to: Option<String>) -> Result<Self> {
        Ok(Self {
            version: 1,
            content_id: hex(&random_id()?),
            request_id: hex(&random_id()?),
            created_at_micros: crate::now_micros()?,
            body,
            reply_to,
        })
    }

    pub fn short_id(&self) -> &str {
        self.content_id.get(..8).unwrap_or(&self.content_id)
    }

    pub fn preview(&self, max_chars: usize) -> String {
        let first = self
            .body
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("");
        let mut preview: String = first
            .trim()
            .chars()
            .take(max_chars)
            .map(|character| {
                if character.is_control() {
                    ' '
                } else {
                    character
                }
            })
            .collect();
        if first.trim().chars().count() > max_chars {
            preview.push('…');
        }
        preview
    }

    fn validate(&self) -> Result<()> {
        if self.version != 1 {
            return Err(Error::Message(format!(
                "unsupported pending record version {}",
                self.version
            )));
        }
        if !is_hex_64(&self.content_id) || !is_hex_64(&self.request_id) {
            return Err(Error::Message(
                "pending record has an invalid content or request ID".into(),
            ));
        }
        if self.body.trim().is_empty() {
            return Err(Error::Message("pending record has an empty body".into()));
        }
        Ok(())
    }
}

fn is_hex_64(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub struct Spool {
    dir: PathBuf,
}

impl Spool {
    pub fn open() -> Result<Self> {
        let root = state_root()?;
        let dir = root.join("requests");
        create_private_dir(&root)?;
        create_private_dir(&dir)?;
        Ok(Self { dir })
    }

    #[cfg(test)]
    fn at(dir: PathBuf) -> Result<Self> {
        create_private_dir(&dir)?;
        Ok(Self { dir })
    }

    pub fn save(&self, pending: &PendingCapture) -> Result<PathBuf> {
        pending.validate()?;
        let path = self.dir.join(format!("{}.json", pending.content_id));
        if path.exists() {
            let existing: PendingCapture = serde_json::from_slice(&fs::read(&path)?)?;
            if existing.request_id == pending.request_id
                && existing.content_id == pending.content_id
                && existing.body == pending.body
                && existing.reply_to == pending.reply_to
                && existing.created_at_micros == pending.created_at_micros
            {
                return Ok(path);
            }
            return Err(Error::Message(format!(
                "a different pending capture already uses {}",
                pending.short_id()
            )));
        }
        let temp = self.dir.join(format!(
            ".{}.{}.tmp",
            pending.content_id,
            hex(&random_id()?)
        ));
        let bytes = serde_json::to_vec_pretty(pending)?;
        {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        fs::rename(&temp, &path)?;
        if let Ok(directory) = fs::File::open(&self.dir) {
            let _ = directory.sync_all();
        }
        Ok(path)
    }

    pub fn list(&self) -> Result<Vec<PendingCapture>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut pending = Vec::new();
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            match fs::read(&path).map_err(Error::from).and_then(|bytes| {
                serde_json::from_slice::<PendingCapture>(&bytes).map_err(Error::from)
            }) {
                Ok(record) if record.validate().is_ok() => pending.push(record),
                Ok(_) | Err(_) => {
                    eprintln!("mote: ignoring malformed pending record {}", path.display())
                }
            }
        }
        pending.sort_by(|left, right| {
            left.created_at_micros
                .cmp(&right.created_at_micros)
                .then_with(|| left.content_id.cmp(&right.content_id))
        });
        Ok(pending)
    }

    pub fn find(&self, prefix: &str) -> Result<PendingCapture> {
        let prefix = prefix.trim().to_ascii_lowercase();
        if prefix.len() < 4
            || prefix.len() > 64
            || !prefix.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(Error::Message(format!(
                "invalid pending ID prefix `{prefix}`"
            )));
        }
        let mut matches: Vec<_> = self
            .list()?
            .into_iter()
            .filter(|record| record.content_id.starts_with(&prefix))
            .collect();
        match matches.len() {
            0 => Err(Error::Message(format!(
                "no pending capture matches `{prefix}`"
            ))),
            1 => Ok(matches.remove(0)),
            _ => Err(Error::Message(format!(
                "`{prefix}` is ambiguous: {}",
                matches
                    .iter()
                    .map(|record| record.content_id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }

    pub fn remove(&self, pending: &PendingCapture) -> Result<()> {
        let path = self.dir.join(format!("{}.json", pending.content_id));
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

fn state_root() -> Result<PathBuf> {
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

fn create_private_dir(path: &Path) -> Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_spool() -> (PathBuf, Spool) {
        let dir =
            std::env::temp_dir().join(format!("mote-spool-test-{}", hex(&random_id().unwrap())));
        let spool = Spool::at(dir.clone()).unwrap();
        (dir, spool)
    }

    #[test]
    fn round_trips_and_is_idempotent() {
        let (dir, spool) = temp_spool();
        let pending = PendingCapture::new("hello\nworld".into(), Some("11".repeat(32))).unwrap();
        let path = spool.save(&pending).unwrap();
        assert!(path.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(&spool.dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        assert_eq!(spool.save(&pending).unwrap(), path);
        let listed = spool.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].body, "hello\nworld");
        assert_eq!(
            spool.find(pending.short_id()).unwrap().request_id,
            pending.request_id
        );
        spool.remove(&pending).unwrap();
        assert!(spool.list().unwrap().is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_a_second_record_for_the_same_content() {
        let (dir, spool) = temp_spool();
        let mut pending = PendingCapture::new("one".into(), None).unwrap();
        spool.save(&pending).unwrap();
        let original = pending.request_id.clone();
        pending.request_id = "22".repeat(32);
        let error = spool.save(&pending).unwrap_err().to_string();
        assert!(error.contains("different pending capture"));
        assert_ne!(pending.request_id, original);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_invalid_records() {
        let (dir, spool) = temp_spool();
        let mut pending = PendingCapture::new("one".into(), None).unwrap();
        pending.content_id = "nope".into();
        assert!(spool.save(&pending).is_err());
        let _ = fs::remove_dir_all(dir);
    }
}
