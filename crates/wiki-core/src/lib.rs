//! Wiki syntax shared by the server, GUI, LSP and the Neovim plugin.
//!
//! The M0 slice only covers embedded blocks: an author writes an `!`-prefixed
//! fenced block, and the app replaces it with the canonical
//! `preset|kinds||id` form that points at a snippet Content.

use std::fmt;

/// Metadata carried by an `!`-prefixed fenced block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FenceSpec {
    /// The preset written by the author, e.g. `hledger`.
    pub preset: String,
    /// Language used for syntax highlighting, e.g. `ledger`.
    pub language: String,
    /// Extra Harmion kinds requested by the fence (`wiki` is implicit).
    pub kinds: Vec<String>,
    /// Canonical Content id, if this is the already-converted form.
    pub id: Option<String>,
}

/// One embedded block found in a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedBlock {
    pub spec: FenceSpec,
    /// Text between the opening and closing fences.
    pub body: String,
    /// Byte offset of the opening fence.
    pub start: usize,
    /// Byte offset just after the closing fence.
    pub end: usize,
    /// Number of backticks in the opening fence.
    pub fence_len: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

/// Resolve a preset to its highlighting language and default kinds.
pub fn resolve_preset(preset: &str) -> (&'static str, &'static [&'static str]) {
    match preset {
        "hledger" | "ledger" => ("ledger", &["ledger"]),
        _ => (Box::leak(preset.to_owned().into_boxed_str()), &[]),
    }
}

/// Parse the info string after `!` + backticks.
///
/// Forms:
/// - `preset`
/// - `preset|kind1,kind2`
/// - `preset|kind1,kind2||<64 hex id>`
/// - `preset||<64 hex id>`
pub fn parse_info(info: &str, line: usize) -> Result<FenceSpec, ParseError> {
    let (head, id) = match info.split_once("||") {
        Some((head, id)) => {
            let id = id.trim();
            if !is_content_id(id) {
                return Err(ParseError {
                    line,
                    message: format!("canonical id must be 64 hex characters, got {id:?}"),
                });
            }
            (head, Some(id.to_owned()))
        }
        None => (info, None),
    };

    let (preset, explicit_kinds) = match head.split_once('|') {
        Some((preset, kinds)) => (preset.trim(), Some(kinds)),
        None => (head.trim(), None),
    };
    if !is_name(preset) {
        return Err(ParseError {
            line,
            message: format!("invalid preset {preset:?}"),
        });
    }

    let (language, default_kinds) = resolve_preset(preset);
    let kinds = match explicit_kinds {
        Some(kinds) => parse_kinds(kinds, line)?,
        None => default_kinds
            .iter()
            .map(|kind| (*kind).to_owned())
            .collect(),
    };

    Ok(FenceSpec {
        preset: preset.to_owned(),
        language: language.to_owned(),
        kinds,
        id,
    })
}

fn parse_kinds(input: &str, line: usize) -> Result<Vec<String>, ParseError> {
    let mut kinds = Vec::new();
    for raw in input.split(',') {
        let kind = raw.trim();
        if kind.is_empty() {
            continue;
        }
        if !is_name(kind) {
            return Err(ParseError {
                line,
                message: format!("invalid kind {kind:?}"),
            });
        }
        if !kinds.iter().any(|existing| existing == kind) {
            kinds.push(kind.to_owned());
        }
    }
    Ok(kinds)
}

fn is_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'/'))
}

fn is_content_id(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Render the canonical opening info for a snippet id.
pub fn canonical_info(spec: &FenceSpec, id: &str) -> String {
    if spec.kinds.is_empty() {
        format!("{}||{}", spec.preset, id)
    } else {
        format!("{}|{}||{}", spec.preset, spec.kinds.join(","), id)
    }
}

/// Render the canonical, body-less block that replaces the authored fence.
pub fn canonical_block(spec: &FenceSpec, id: &str) -> String {
    format!("!```{}\n```\n", canonical_info(spec, id))
}

/// Return all `!`-prefixed embedded blocks in a document.
pub fn scan_embedded_blocks(doc: &str) -> Result<Vec<EmbeddedBlock>, ParseError> {
    let mut blocks = Vec::new();
    let mut offset = 0usize;
    let mut line_no = 1usize;
    let mut lines = doc.split_inclusive('\n').peekable();

    while let Some(line) = lines.next() {
        let line_start = offset;
        offset += line.len();

        let Some((fence_len, info)) = prefixed_fence(line) else {
            line_no += 1;
            continue;
        };

        let spec = parse_info(info, line_no)?;
        let body_start = offset;
        let mut body_end = None;
        let mut end = offset;

        while let Some(body_line) = lines.next() {
            let this_start = offset;
            offset += body_line.len();
            line_no += 1;
            if is_closing_fence(body_line, fence_len) {
                body_end = Some(this_start);
                end = offset;
                break;
            }
        }

        let Some(body_end) = body_end else {
            return Err(ParseError {
                line: line_no,
                message: "unclosed `!` fenced block".to_owned(),
            });
        };

        let body = doc[body_start..body_end].to_owned();
        if spec.id.is_some() && !body.trim().is_empty() {
            return Err(ParseError {
                line: line_no,
                message: "canonical `!` fenced block must not have a body".to_owned(),
            });
        }

        blocks.push(EmbeddedBlock {
            spec,
            body,
            start: line_start,
            end,
            fence_len,
        });
        line_no += 1;
    }

    Ok(blocks)
}

fn prefixed_fence(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start_matches([' ', '\t']);
    let rest = trimmed.strip_prefix('!')?;
    let backticks = rest.bytes().take_while(|b| *b == b'`').count();
    if backticks < 3 {
        return None;
    }
    let info = rest[backticks..].trim();
    Some((backticks, info))
}

fn is_closing_fence(line: &str, fence_len: usize) -> bool {
    let trimmed = line
        .trim_start_matches([' ', '\t'])
        .trim_end_matches(['\r', '\n']);
    let backticks = trimmed.bytes().take_while(|b| *b == b'`').count();
    backticks >= fence_len && trimmed[backticks..].trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "3f2a9c1e00000000000000000000000000000000000000000000000000000001";

    #[test]
    fn ledger_preset_has_ledger_language_and_kind() {
        let spec = parse_info("ledger", 1).unwrap();
        assert_eq!(spec.preset, "ledger");
        assert_eq!(spec.language, "ledger");
        assert_eq!(spec.kinds, ["ledger"]);
        assert_eq!(spec.id, None);
    }

    #[test]
    fn hledger_preset_resolves_language_and_explicit_kinds() {
        let spec = parse_info("hledger|ledger,free", 1).unwrap();
        assert_eq!(spec.preset, "hledger");
        assert_eq!(spec.language, "ledger");
        assert_eq!(spec.kinds, ["ledger", "free"]);
    }

    #[test]
    fn unknown_preset_has_no_extra_kinds() {
        let spec = parse_info("rust", 1).unwrap();
        assert_eq!(spec.language, "rust");
        assert!(spec.kinds.is_empty());
    }

    #[test]
    fn canonical_form_with_kinds() {
        let spec = parse_info(&format!("hledger|ledger,free||{ID}"), 1).unwrap();
        assert_eq!(spec.id.as_deref(), Some(ID));
        assert_eq!(
            canonical_info(&spec, ID),
            format!("hledger|ledger,free||{ID}")
        );
    }

    #[test]
    fn canonical_form_without_kinds() {
        let spec = parse_info(&format!("rust||{ID}"), 1).unwrap();
        assert!(spec.kinds.is_empty());
        assert_eq!(canonical_info(&spec, ID), format!("rust||{ID}"));
    }

    #[test]
    fn scans_a_loose_block() {
        let doc = "before\n!```hledger|ledger,free\n2026-11-16 * 買い物\n```\nafter\n";
        let blocks = scan_embedded_blocks(doc).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].body, "2026-11-16 * 買い物\n");
        assert_eq!(blocks[0].spec.kinds, ["ledger", "free"]);
        assert_eq!(
            &doc[blocks[0].start..blocks[0].end],
            "!```hledger|ledger,free\n2026-11-16 * 買い物\n```\n"
        );
    }

    #[test]
    fn scans_canonical_block_with_empty_body() {
        let doc = format!("!```hledger|ledger||{ID}\n```\n");
        let blocks = scan_embedded_blocks(&doc).unwrap();
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].body.is_empty());
        assert_eq!(blocks[0].spec.id.as_deref(), Some(ID));
    }

    #[test]
    fn ignores_plain_fences() {
        let doc = "```rust\nfn main() {}\n```\n";
        assert!(scan_embedded_blocks(doc).unwrap().is_empty());
    }

    #[test]
    fn scans_multiple_blocks_and_keeps_offsets() {
        let doc = "a\n!```ledger\none\n```\nb\n!```rust|code\nfn x() {}\n```\nc\n";
        let blocks = scan_embedded_blocks(doc).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].body, "one\n");
        assert_eq!(blocks[1].body, "fn x() {}\n");
        assert!(blocks[0].end <= blocks[1].start);
    }

    #[test]
    fn rejects_unclosed_block() {
        let err = scan_embedded_blocks("!```ledger\nbody\n").unwrap_err();
        assert!(err.message.contains("unclosed"));
    }

    #[test]
    fn rejects_invalid_id() {
        let err = parse_info("ledger||not-hex", 1).unwrap_err();
        assert!(err.message.contains("64 hex"));
    }

    #[test]
    fn canonical_block_has_no_body() {
        let spec = parse_info("hledger|ledger,free", 1).unwrap();
        assert_eq!(
            canonical_block(&spec, ID),
            format!("!```hledger|ledger,free||{ID}\n```\n")
        );
    }
}
