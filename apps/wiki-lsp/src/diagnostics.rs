//! Document diagnostics derived from the local buffer and the server index.
//!
//! Local checks (fence syntax) always run. Index-dependent checks (unresolved
//! links, missing snippets, duplicate titles) only run when the last `list`
//! call succeeded, so a stopped server cannot produce bogus "unresolved"
//! warnings.

use harmion_wiki_core::{normalize_target, parse_info, parse_links, scan_embedded_blocks};

use crate::protocol::{Diagnostic, Position, Range, SEVERITY_ERROR, SEVERITY_WARNING};
use crate::server::ContentSummary;
use crate::text;

pub fn compute(
    text: &str,
    content_id: &str,
    index: &[ContentSummary],
    server_available: bool,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    if let Err(error) = scan_embedded_blocks(text) {
        let line = error.line.saturating_sub(1) as u32;
        diagnostics.push(Diagnostic {
            range: text::range_for_line(text, line),
            severity: SEVERITY_ERROR,
            code: "invalid-fence",
            source: "wiki",
            message: format!("invalid wiki fence: {}", error.message),
        });
    }

    if server_available {
        for link in parse_links(text) {
            if !resolves(&link.target, index) {
                diagnostics.push(Diagnostic {
                    range: text::range_for_span(text, link.start, link.end),
                    severity: SEVERITY_WARNING,
                    code: "unresolved-link",
                    source: "wiki",
                    message: format!(
                        "unresolved wiki {}: `{}`",
                        if link.embed { "embed" } else { "link" },
                        link.target
                    ),
                });
            }
        }

        for (start, end, id) in canonical_fence_ids(text) {
            if !index.iter().any(|content| content.id == id) {
                diagnostics.push(Diagnostic {
                    range: text::range_for_span(text, start, end),
                    severity: SEVERITY_ERROR,
                    code: "missing-snippet",
                    source: "wiki",
                    message: format!("canonical fence references missing snippet `{id}`"),
                });
            }
        }

        if let Some(current) = index.iter().find(|content| content.id == content_id) {
            let lowered = current.title.to_lowercase();
            let count = index
                .iter()
                .filter(|content| content.title.to_lowercase() == lowered)
                .count();
            if count > 1 {
                diagnostics.push(Diagnostic {
                    range: Range::at(Position::new(0, 0)),
                    severity: SEVERITY_WARNING,
                    code: "duplicate-title",
                    source: "wiki",
                    message: format!(
                        "duplicate title `{}` is used by {count} Contents",
                        current.title
                    ),
                });
            }
        }
    }

    diagnostics.sort_by(|left, right| {
        left.range
            .cmp(&right.range)
            .then_with(|| left.message.cmp(&right.message))
    });
    diagnostics.dedup_by(|left, right| {
        left.range == right.range && left.code == right.code && left.message == right.message
    });
    diagnostics
}

fn resolves(target: &str, index: &[ContentSummary]) -> bool {
    let target = normalize_target(target);
    let lowered = target.to_lowercase();
    index.iter().any(|content| {
        content.id == target || content.title == target || content.title.to_lowercase() == lowered
    })
}

/// Lenient scan for canonical fence ids. It deliberately does not fail on the
/// first malformed fence so a missing snippet is still reported alongside the
/// syntax error.
fn canonical_fence_ids(text: &str) -> Vec<(usize, usize, String)> {
    let mut found = Vec::new();
    let mut offset = 0usize;
    let mut in_fence = None::<usize>;
    let mut plain_fence = None::<usize>;

    for (line_index, line) in text.split_inclusive('\n').enumerate() {
        let line_start = offset;
        offset += line.len();
        let line_end = line_start + line.trim_end_matches(['\r', '\n']).len();

        if let Some(fence_len) = plain_fence {
            if is_closing_fence(line, fence_len) {
                plain_fence = None;
            }
            continue;
        }
        if let Some(fence_len) = plain_fence_start(line) {
            plain_fence = Some(fence_len);
            continue;
        }
        if let Some(fence_len) = in_fence {
            if is_closing_fence(line, fence_len) {
                in_fence = None;
            }
            continue;
        }

        let Some((fence_len, info_start)) = prefixed_fence(line, line_start) else {
            continue;
        };
        in_fence = Some(fence_len);
        let info = line[info_start - line_start..]
            .trim_end_matches(['\r', '\n'])
            .trim();
        if let Ok(spec) = parse_info(info, line_index + 1) {
            if let Some(id) = spec.id {
                found.push((info_start, line_end, id));
            }
        }
    }
    found
}

fn plain_fence_start(line: &str) -> Option<usize> {
    let trimmed = line.trim_start_matches([' ', '\t']);
    let backticks = trimmed.bytes().take_while(|byte| *byte == b'`').count();
    (backticks >= 3).then_some(backticks)
}

fn prefixed_fence(line: &str, line_start: usize) -> Option<(usize, usize)> {
    let trimmed = line.trim_start_matches([' ', '\t']);
    let indent = line.len() - trimmed.len();
    let rest = trimmed.strip_prefix('!')?;
    let backticks = rest.bytes().take_while(|byte| *byte == b'`').count();
    if backticks < 3 {
        return None;
    }
    Some((backticks, line_start + indent + 1 + backticks))
}

fn is_closing_fence(line: &str, fence_len: usize) -> bool {
    let trimmed = line
        .trim_start_matches([' ', '\t'])
        .trim_end_matches(['\r', '\n']);
    let backticks = trimmed.bytes().take_while(|byte| *byte == b'`').count();
    backticks >= fence_len && trimmed[backticks..].trim().is_empty()
}
