//! Small LSP value types shared by the engine and the feature modules.
//!
//! The server deliberately keeps the protocol surface small: JSON-RPC is
//! handled with [`serde_json::Value`], while these structs type the pieces we
//! construct ourselves.

use serde::{Deserialize, Serialize};

pub const SEVERITY_ERROR: u8 = 1;
pub const SEVERITY_WARNING: u8 = 2;

pub const COMPLETION_TEXT: u8 = 1;
pub const COMPLETION_FUNCTION: u8 = 3;
pub const COMPLETION_KEYWORD: u8 = 14;
pub const COMPLETION_REFERENCE: u8 = 18;
pub const COMPLETION_ENUM_MEMBER: u8 = 20;

/// A zero-based LSP position. `character` counts UTF-16 code units.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

impl Position {
    pub fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

impl Range {
    pub fn new(start: Position, end: Position) -> Self {
        Self { start, end }
    }

    pub fn at(position: Position) -> Self {
        Self {
            start: position,
            end: position,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Location {
    pub uri: String,
    pub range: Range,
}

#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub range: Range,
    pub severity: u8,
    pub code: &'static str,
    pub source: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionItem {
    pub label: String,
    pub kind: u8,
    pub detail: String,
    pub insert_text: String,
}
