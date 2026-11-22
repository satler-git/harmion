//! Text offsets, LSP positions, and the small syntactic contexts needed by
//! completion and hover.
//!
//! `wiki-core` reports byte offsets. LSP positions count UTF-16 code units and
//! lines, so every conversion goes through this module. Japanese text is a
//! first-class case here.

use harmion_wiki_core::{
    EmbeddedBlock, InterpreterCall, WikiLink, parse_interpreter_calls, parse_links,
    scan_embedded_blocks,
};

use crate::protocol::{Position, Range};

/// Convert a byte offset to an LSP position. Offsets that are not on a char
/// boundary are rounded down.
pub fn position_at(text: &str, byte: usize) -> Position {
    let mut byte = byte.min(text.len());
    while !text.is_char_boundary(byte) {
        byte -= 1;
    }

    let mut line = 0u32;
    let mut line_start = 0usize;
    for (index, ch) in text.char_indices() {
        if index >= byte {
            break;
        }
        if ch == '\n' {
            line += 1;
            line_start = index + 1;
        }
    }

    let character = text[line_start..byte]
        .chars()
        .map(|ch| ch.len_utf16() as u32)
        .sum();
    Position::new(line, character)
}

/// Convert an LSP position to a byte offset. Out-of-range positions clamp to
/// the end of the document/line.
pub fn offset_at(text: &str, position: Position) -> usize {
    let mut line = 0u32;
    let mut line_start = 0usize;
    if position.line > 0 {
        let mut found = false;
        for (index, ch) in text.char_indices() {
            if ch == '\n' {
                line += 1;
                if line == position.line {
                    line_start = index + 1;
                    found = true;
                    break;
                }
            }
        }
        if !found {
            return text.len();
        }
    }

    let line_end = text[line_start..]
        .find('\n')
        .map(|relative| line_start + relative)
        .unwrap_or(text.len());

    let mut byte = line_start;
    let mut units = 0u32;
    for (relative, ch) in text[line_start..line_end].char_indices() {
        let width = ch.len_utf16() as u32;
        if units + width > position.character {
            break;
        }
        units += width;
        byte = line_start + relative + ch.len_utf8();
    }
    byte
}

/// Byte bounds of a zero-based line, excluding the trailing newline.
pub fn line_bounds(text: &str, line: u32) -> Option<(usize, usize)> {
    let mut current = 0u32;
    let mut start = 0usize;
    for (index, ch) in text.char_indices() {
        if ch == '\n' {
            if current == line {
                return Some((start, index));
            }
            current += 1;
            start = index + 1;
        }
    }
    if current == line {
        Some((start, text.len()))
    } else {
        None
    }
}

pub fn range_for_span(text: &str, start: usize, end: usize) -> Range {
    Range::new(position_at(text, start), position_at(text, end))
}

/// Range covering a zero-based line. Unknown lines become an empty range at
/// the end of the document.
pub fn range_for_line(text: &str, line: u32) -> Range {
    match line_bounds(text, line) {
        Some((start, end)) => range_for_span(text, start, end),
        None => {
            let end = position_at(text, text.len());
            Range::at(end)
        }
    }
}

/// The range covering only the info string of an embedded fence (everything
/// after `!` + backticks on the opening line).
pub fn fence_info_range(text: &str, block: &EmbeddedBlock) -> Range {
    let line_end = text[block.start..]
        .find('\n')
        .map(|relative| block.start + relative)
        .unwrap_or(text.len());
    let line = &text[block.start..line_end];
    let info_start = line
        .find('!')
        .map(|bang| block.start + bang + 1 + block.fence_len)
        .unwrap_or(block.start);
    range_for_span(text, info_start.min(line_end), line_end)
}

/// A link whose `[[` is open at the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingLink {
    pub embed: bool,
    /// Text typed inside the link, up to the cursor.
    pub typed: String,
    /// True when the cursor is after `|` (label area; no title completion).
    pub in_label: bool,
    /// Byte offset of the first `[`.
    pub start: usize,
}

/// Detect an unclosed `[[`/`![[` immediately before `byte`.
pub fn pending_link_at(text: &str, byte: usize) -> Option<PendingLink> {
    let byte = byte.min(text.len());
    let line_start = text[..byte].rfind('\n').map(|index| index + 1).unwrap_or(0);
    let prefix = &text[line_start..byte];
    let open_relative = prefix.rfind("[[")?;
    let open = line_start + open_relative;
    let after = &prefix[open_relative + 2..];
    if after.contains("]]") {
        return None;
    }

    let embed = prefix[..open_relative].ends_with('!');
    let in_label = after.contains('|');
    let typed = if let Some((_, label)) = after.rsplit_once('|') {
        label
    } else {
        after
    };

    Some(PendingLink {
        embed,
        typed: typed.trim().to_owned(),
        in_label,
        start: open,
    })
}

/// A provider name being typed after `![`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingProvider {
    pub typed: String,
    pub start: usize,
}

/// Detect an unclosed `![provider` (not an embed `![[`).
pub fn pending_provider_at(text: &str, byte: usize) -> Option<PendingProvider> {
    let byte = byte.min(text.len());
    let line_start = text[..byte].rfind('\n').map(|index| index + 1).unwrap_or(0);
    let prefix = &text[line_start..byte];
    let open_relative = prefix.rfind("![")?;
    let after = &prefix[open_relative + 2..];
    if after.starts_with('[') || after.contains(")]") || after.contains('(') || after.contains(']')
    {
        return None;
    }

    Some(PendingProvider {
        typed: after.trim().to_owned(),
        start: line_start + open_relative,
    })
}

/// The part of an embedded fence info string being typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingFence {
    Preset { typed: String },
    Kind { typed: String, preset: String },
}

/// Detect a cursor inside the info string of an `!```` fence.
pub fn pending_fence_at(text: &str, byte: usize) -> Option<PendingFence> {
    let byte = byte.min(text.len());
    let line_start = text[..byte].rfind('\n').map(|index| index + 1).unwrap_or(0);
    let prefix = &text[line_start..byte];
    let trimmed_start = prefix.len() - prefix.trim_start_matches([' ', '\t']).len();
    let trimmed = &prefix[trimmed_start..];
    let rest = trimmed.strip_prefix('!')?;
    let backticks = rest.bytes().take_while(|b| *b == b'`').count();
    if backticks < 3 {
        return None;
    }

    let info_start = trimmed_start + 1 + backticks;
    if byte - line_start < info_start {
        return None;
    }
    let info = &prefix[info_start..];
    if info.contains("||") {
        return None;
    }

    if let Some((preset, kinds)) = info.rsplit_once('|') {
        let typed = kinds.rsplit(',').next().unwrap_or("").trim().to_owned();
        Some(PendingFence::Kind {
            typed,
            preset: preset.trim().to_owned(),
        })
    } else {
        Some(PendingFence::Preset {
            typed: info.trim().to_owned(),
        })
    }
}

/// The wiki link under the cursor, if any.
pub fn link_at(text: &str, byte: usize) -> Option<WikiLink> {
    parse_links(text)
        .into_iter()
        .find(|link| link.start <= byte && byte <= link.end)
}

/// The interpreter call under the cursor, if any.
pub fn interpreter_at(text: &str, byte: usize) -> Option<InterpreterCall> {
    parse_interpreter_calls(text)
        .into_iter()
        .find(|call| call.start <= byte && byte <= call.end)
}

/// The embedded fence under the cursor, if any.
pub fn fence_at(text: &str, byte: usize) -> Option<EmbeddedBlock> {
    scan_embedded_blocks(text)
        .ok()?
        .into_iter()
        .find(|block| block.start <= byte && byte <= block.end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_and_offset_are_utf16_aware() {
        let text = "日本語 [[家計]]\n次の行";
        let byte = text.find("家計").unwrap() + "家".len();
        let position = position_at(text, byte);
        assert_eq!(position.line, 0);
        // 日本語(3) + space(1) + [[(2) + 家(1) = 7 UTF-16 units
        assert_eq!(position.character, 7);
        assert_eq!(offset_at(text, position), byte);
    }

    #[test]
    fn line_bounds_and_range() {
        let text = "a\nbb\n";
        assert_eq!(line_bounds(text, 0), Some((0, 1)));
        assert_eq!(line_bounds(text, 1), Some((2, 4)));
        assert_eq!(line_bounds(text, 2), Some((5, 5)));
        assert_eq!(line_bounds(text, 3), None);
        let range = range_for_line(text, 1);
        assert_eq!(range.start, Position::new(1, 0));
        assert_eq!(range.end, Position::new(1, 2));
    }

    #[test]
    fn detects_pending_links() {
        let text = "a [[家計";
        let pending = pending_link_at(text, text.len()).unwrap();
        assert!(!pending.embed);
        assert_eq!(pending.typed, "家計");
        assert!(!pending.in_label);

        let text = "![[買い物";
        let pending = pending_link_at(text, text.len()).unwrap();
        assert!(pending.embed);
        assert_eq!(pending.typed, "買い物");

        let text = "[[target|lab";
        let pending = pending_link_at(text, text.len()).unwrap();
        assert!(pending.in_label);
        assert_eq!(pending.typed, "lab");

        let text = "[[closed]] ";
        assert!(pending_link_at(text, text.len()).is_none());
    }

    #[test]
    fn detects_pending_provider_and_fence() {
        let text = "![ledger/bal";
        let pending = pending_provider_at(text, text.len()).unwrap();
        assert_eq!(pending.typed, "ledger/bal");

        assert!(pending_provider_at("![[買い物", "![[買い物".len()).is_none());
        assert!(pending_provider_at("![ledger/balance(", "![ledger/balance(".len()).is_none());

        let text = "!```hled";
        assert_eq!(
            pending_fence_at(text, text.len()),
            Some(PendingFence::Preset {
                typed: "hled".to_owned()
            })
        );

        let text = "!```hledger|ledger,fr";
        assert_eq!(
            pending_fence_at(text, text.len()),
            Some(PendingFence::Kind {
                typed: "fr".to_owned(),
                preset: "hledger".to_owned()
            })
        );

        assert!(
            pending_fence_at("!```hledger|ledger||abc", "!```hledger|ledger||abc".len()).is_none()
        );
    }

    #[test]
    fn finds_links_and_fences_at_offsets() {
        let text = "x [[p-home]] y\n\n!```ledger\nbody\n```\n";
        let link = link_at(text, text.find("p-home").unwrap()).unwrap();
        assert_eq!(link.target, "p-home");
        let fence = fence_at(text, text.find("ledger").unwrap()).unwrap();
        assert_eq!(fence.spec.preset, "ledger");
    }
}
