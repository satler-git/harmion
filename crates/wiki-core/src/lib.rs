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
    let mut plain_fence_len = None::<usize>;
    let mut lines = doc.split_inclusive('\n').peekable();

    while let Some(line) = lines.next() {
        let line_start = offset;
        offset += line.len();

        if let Some(fence_len) = plain_fence_len {
            if is_closing_fence(line, fence_len) {
                plain_fence_len = None;
            }
            line_no += 1;
            continue;
        }
        if let Some((fence_len, _)) = plain_fence(line) {
            plain_fence_len = Some(fence_len);
            line_no += 1;
            continue;
        }

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

/// A `[[target|label]]` or `![[target|label]]` link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WikiLink {
    pub target: String,
    pub label: Option<String>,
    pub embed: bool,
    pub line: usize,
    pub start: usize,
    pub end: usize,
}

/// An `![provider(args)]` interpreter call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterpreterCall {
    pub provider: String,
    pub args: String,
    pub line: usize,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineItem {
    pub level: u8,
    pub title: String,
    pub line: usize,
}

/// Links outside fenced code and inline code. Canonical fences contain no
/// links, and ordinary code fences are literal text, so both are skipped.
pub fn parse_links(doc: &str) -> Vec<WikiLink> {
    let mut links = Vec::new();
    let mut in_fence = None::<usize>;
    let mut offset = 0usize;

    for (line_index, line) in doc.split_inclusive('\n').enumerate() {
        let line_no = line_index + 1;
        if let Some(fence_len) = in_fence {
            if is_closing_fence(line, fence_len) {
                in_fence = None;
            }
            offset += line.len();
            continue;
        }
        if let Some((fence_len, _)) = prefixed_fence(line).or_else(|| plain_fence(line)) {
            in_fence = Some(fence_len);
            offset += line.len();
            continue;
        }

        let bytes = line.as_bytes();
        let mut i = 0usize;
        let mut code = false;
        while i < bytes.len() {
            if bytes[i] == b'`' {
                code = !code;
                i += 1;
                continue;
            }
            if code {
                i += 1;
                continue;
            }
            let (embed, open_len) = if bytes[i..].starts_with(b"![[") {
                (true, 3)
            } else if bytes[i..].starts_with(b"[[") {
                (false, 2)
            } else {
                i += 1;
                continue;
            };
            let content_start = i + open_len;
            let Some(close_rel) = line[content_start..].find("]]") else {
                break;
            };
            let content_end = content_start + close_rel;
            let content = &line[content_start..content_end];
            let (target, label) = match content.split_once('|') {
                Some((target, label)) => (target.trim(), Some(label.trim().to_owned())),
                None => (content.trim(), None),
            };
            if !target.is_empty() {
                links.push(WikiLink {
                    target: target.to_owned(),
                    label,
                    embed,
                    line: line_no,
                    start: offset + i,
                    end: offset + content_end + 2,
                });
            }
            i = content_end + 2;
        }
        offset += line.len();
    }
    links
}

/// Interpreter calls outside fenced and inline code.
pub fn parse_interpreter_calls(doc: &str) -> Vec<InterpreterCall> {
    let mut calls = Vec::new();
    let mut in_fence = None::<usize>;
    let mut offset = 0usize;

    for (line_index, line) in doc.split_inclusive('\n').enumerate() {
        let line_no = line_index + 1;
        if let Some(fence_len) = in_fence {
            if is_closing_fence(line, fence_len) {
                in_fence = None;
            }
            offset += line.len();
            continue;
        }
        if let Some((fence_len, _)) = prefixed_fence(line).or_else(|| plain_fence(line)) {
            in_fence = Some(fence_len);
            offset += line.len();
            continue;
        }

        let bytes = line.as_bytes();
        let mut i = 0usize;
        let mut code = false;
        while i + 2 < bytes.len() {
            if bytes[i] == b'`' {
                code = !code;
                i += 1;
                continue;
            }
            if code || !bytes[i..].starts_with(b"![") || bytes.get(i + 2) == Some(&b'[') {
                i += 1;
                continue;
            }
            let Some(open_rel) = line[i + 2..].find('(') else {
                i += 1;
                continue;
            };
            let open = i + 2 + open_rel;
            let Some(close_rel) = line[open + 1..].find(")]") else {
                i += 1;
                continue;
            };
            let close = open + 1 + close_rel;
            let provider = line[i + 2..open].trim();
            if !provider.is_empty()
                && provider
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '.'))
            {
                calls.push(InterpreterCall {
                    provider: provider.to_owned(),
                    args: line[open + 1..close].trim().to_owned(),
                    line: line_no,
                    start: offset + i,
                    end: offset + close + 2,
                });
            }
            i = close + 2;
        }
        offset += line.len();
    }
    calls
}

/// Markdown headings outside code fences.
pub fn outline(doc: &str) -> Vec<OutlineItem> {
    let mut items = Vec::new();
    let mut in_fence = None::<usize>;

    for (line_index, line) in doc.split_inclusive('\n').enumerate() {
        if let Some(fence_len) = in_fence {
            if is_closing_fence(line, fence_len) {
                in_fence = None;
            }
            continue;
        }
        if let Some((fence_len, _)) = prefixed_fence(line).or_else(|| plain_fence(line)) {
            in_fence = Some(fence_len);
            continue;
        }
        let trimmed = line.trim_start();
        let level = trimmed.bytes().take_while(|b| *b == b'#').count();
        if !(1..=6).contains(&level) {
            continue;
        }
        let title = trimmed[level..].trim().trim_end_matches('#').trim();
        if !title.is_empty() {
            items.push(OutlineItem {
                level: level as u8,
                title: title.to_owned(),
                line: line_index + 1,
            });
        }
    }
    items
}

/// A suggestion reached through a page that links to the current page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TwoHopLink {
    pub via: String,
    pub target: String,
}

#[derive(Debug, Clone, Default)]
pub struct LinkIndex {
    pub outgoing: std::collections::BTreeMap<String, Vec<String>>,
    pub incoming: std::collections::BTreeMap<String, Vec<String>>,
}

/// Normalize a stored link target for indexing: strip the canonical `id:`
/// prefix and an optional `#fragment`, then trim. Titles and aliases pass through.
pub fn normalize_target(target: &str) -> &str {
    let target = target.trim();
    let target = target.strip_prefix("id:").unwrap_or(target);
    target.split('#').next().unwrap_or(target).trim()
}

impl LinkIndex {
    pub fn from_documents<I>(documents: I) -> Self
    where
        I: IntoIterator<Item = (String, String)>,
    {
        let mut index = Self::default();
        for (id, body) in documents {
            let mut seen = std::collections::BTreeSet::new();
            for link in parse_links(&body) {
                let target = normalize_target(&link.target);
                if target.is_empty() || !seen.insert(target.to_owned()) {
                    continue;
                }
                index
                    .outgoing
                    .entry(id.clone())
                    .or_default()
                    .push(target.to_owned());
                index
                    .incoming
                    .entry(target.to_owned())
                    .or_default()
                    .push(id.clone());
            }
        }
        index
    }

    pub fn backlinks(&self, id: &str) -> &[String] {
        self.incoming.get(id).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn outgoing_links(&self, id: &str) -> &[String] {
        self.outgoing.get(id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Pages reachable from a backlink, excluding the page itself and its
    /// direct outgoing links. Backlinks themselves can be useful suggestions.
    pub fn two_hop(&self, id: &str) -> Vec<TwoHopLink> {
        let direct: std::collections::BTreeSet<&str> =
            self.outgoing_links(id).iter().map(String::as_str).collect();
        let mut seen = std::collections::BTreeSet::new();
        let mut out = Vec::new();
        for via in self.backlinks(id) {
            for target in self.outgoing_links(via) {
                if target == id || direct.contains(target.as_str()) {
                    continue;
                }
                if seen.insert(target.clone()) {
                    out.push(TwoHopLink {
                        via: via.clone(),
                        target: target.clone(),
                    });
                }
            }
        }
        out
    }
}

fn plain_fence(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start_matches([' ', '\t']);
    let backticks = trimmed.bytes().take_while(|b| *b == b'`').count();
    if backticks < 3 {
        return None;
    }
    Some((backticks, trimmed[backticks..].trim()))
}

#[cfg(test)]
mod link_tests {
    use super::*;

    #[test]
    fn parses_plain_and_embedded_links() {
        let doc = "see [[p-home|家計]] and ![[journal]] and [[2026年の目標]]";
        let links = parse_links(doc);
        assert_eq!(links.len(), 3);
        assert_eq!(links[0].target, "p-home");
        assert_eq!(links[0].label.as_deref(), Some("家計"));
        assert!(links[1].embed);
        assert_eq!(links[2].target, "2026年の目標");
    }

    #[test]
    fn skips_links_in_code() {
        let doc = "before [[a]]\n```rust\n[[not a link]]\n```\nafter [[b]]\n!```ledger\n[[also not]]\n```\n";
        let links = parse_links(doc);
        assert_eq!(
            links
                .iter()
                .map(|link| link.target.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
    }

    #[test]
    fn skips_inline_code_and_interpreter_calls() {
        let doc = "`[[inline]]` [[real]] ![ledger/balance(query(before: 2026-11-16))]";
        let links = parse_links(doc);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "real");
        let calls = parse_interpreter_calls(doc);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].provider, "ledger/balance");
        assert_eq!(calls[0].args, "query(before: 2026-11-16)");
    }

    #[test]
    fn ignores_embedded_fences_inside_plain_code() {
        let doc = "````markdown\n!```rust\nfn main() {}\n```\n````\n";
        assert!(scan_embedded_blocks(doc).unwrap().is_empty());
    }

    #[test]
    fn extracts_outline() {
        let doc = "# 家計\n\n## 今月\n\n```rust\n# not heading\n```\n### 残高\n";
        let items = outline(doc);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].title, "家計");
        assert_eq!(items[1].level, 2);
        assert_eq!(items[1].title, "今月");
        assert_eq!(items[2].level, 3);
        assert_eq!(items[2].title, "残高");
    }

    #[test]
    fn canonical_ids_and_fragments_are_normalized() {
        let index =
            LinkIndex::from_documents([("a".to_owned(), "[[id:b|B]] [[id:b#section]]".to_owned())]);
        assert_eq!(index.backlinks("b"), ["a"]);
        assert_eq!(index.outgoing_links("a"), ["b"]);
        assert_eq!(normalize_target(" id:b#x "), "b");
    }

    #[test]
    fn indexes_backlinks_and_two_hop() {
        let index = LinkIndex::from_documents([
            ("a".to_owned(), "[[b]] [[c]]".to_owned()),
            ("b".to_owned(), "[[c]] [[d]]".to_owned()),
            ("c".to_owned(), "[[a]]".to_owned()),
        ]);
        assert_eq!(index.backlinks("c"), ["a", "b"]);
        let hops = index.two_hop("c");
        assert!(hops.iter().any(|hop| hop.via == "b" && hop.target == "d"));
        assert!(
            hops.iter()
                .all(|hop| hop.target != "c" && hop.target != "a")
        );
    }
}
