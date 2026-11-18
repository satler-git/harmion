//! Versioned record stream for owner data exports.

use crate::{
    app_view::{field_json, value_json},
    hex,
};
use harmion_domain::{Action, ContentId, Field, Graph, Life, Update, UpdateId};
use serde_json::{Value as Json, json};
use std::io::{self, Write};

pub const FORMAT: &str = "harmion-export";
pub const VERSION: u32 = 1;
pub const SCOPE: &str = "current-conflicts-and-deletions";
pub const CONTENT_TYPE: &str = "application/x-ndjson";
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

pub struct ExportWriter<W> {
    output: W,
    count: u64,
}

impl<W: Write> ExportWriter<W> {
    pub fn start(mut output: W) -> io::Result<Self> {
        line(
            &mut output,
            &json!({"type":"header","format":FORMAT,"version":VERSION,"scope":SCOPE}),
        )?;
        output.flush()?;
        Ok(Self { output, count: 0 })
    }

    pub fn finish(mut self) -> io::Result<()> {
        line(
            &mut self.output,
            &json!({"type":"end","records":self.count}),
        )?;
        self.output.flush()
    }

    fn emit(&mut self, value: Json) -> io::Result<()> {
        line(&mut self.output, &value)?;
        self.count += 1;
        Ok(())
    }

    pub fn content(&mut self, graph: &Graph, id: ContentId) -> io::Result<()> {
        let snapshot = graph.snapshot(id).ok_or_else(corrupt)?;
        let content_id = hex(&id.0);
        self.emit(json!({
            "type":"content", "content_id":content_id,
            "life": match snapshot.life {
                Life::Live => "live",
                Life::Deleted => "deleted",
                Life::Conflict => "conflict",
            },
            "last_edited_at": snapshot.last_edited_at.to_string(),
        }))?;
        for head in &snapshot.basis {
            self.emit(json!({
                "type":"existence_head", "content_id":content_id,
                "revision":revision(get(graph, *head)?),
            }))?;
        }
        for (field, state) in &snapshot.fields {
            let selected = get(graph, state.selected)?;
            if snapshot.life != Life::Deleted {
                if *field == Field::Body {
                    self.emit(json!({
                        "type":"body", "content_id":content_id, "text":snapshot.content.body,
                        "revision":revision(selected),
                    }))?;
                } else {
                    self.emit(json!({
                        "type":"field", "content_id":content_id, "field":field_json(field),
                        "value":value_json(&state.selected_candidate().value),
                        "revision":revision(selected),
                    }))?;
                }
            }
            if state.is_conflicted() {
                for candidate in &state.candidates {
                    for version in &candidate.versions {
                        self.emit(json!({
                            "type":"candidate", "content_id":content_id,
                            "field":field_json(field), "value":value_json(&candidate.value),
                            "revision":revision(get(graph, *version)?),
                            "selected":*version == state.selected,
                        }))?;
                    }
                }
            }
        }
        for update in graph
            .updates()
            .filter(|u| u.content_id == id && u.action == Action::Delete)
        {
            self.emit(json!({
                "type":"deletion", "content_id":content_id,
                "revision": revision(update),
                "basis": update.basis.iter().map(|id| hex(&id.0)).collect::<Vec<_>>(),
            }))?;
        }
        Ok(())
    }
}

fn corrupt() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid export snapshot")
}

fn get(graph: &Graph, id: UpdateId) -> io::Result<&Update> {
    graph.update(id).ok_or_else(corrupt)
}

fn revision(update: &Update) -> Json {
    json!({
        "id":hex(&update.update_id.0),
        "edited_at":update.edited_at.to_string(),
        "action":match update.action {
            Action::Create => "create",
            Action::Edit => "edit",
            Action::Delete => "delete",
            Action::Keep => "keep",
            Action::Restore => "restore",
        },
    })
}

fn line(output: &mut impl Write, value: &Json) -> io::Result<()> {
    serde_json::to_writer(&mut *output, value)?;
    output.write_all(b"\n")
}
