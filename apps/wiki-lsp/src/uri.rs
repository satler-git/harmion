//! `wiki://<content-id>` virtual document URIs.

/// A parsed wiki URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WikiUri {
    /// Content id, or `new/<uuid>` for an unsaved page.
    pub id: String,
    /// True for the temporary `wiki://new/<uuid>` form.
    pub is_new: bool,
}

/// Parse a `wiki://<content-id>` URI. `None` for other schemes or malformed
/// values, so the LSP can ignore buffers that are not wiki documents.
pub fn parse_wiki_uri(uri: &str) -> Option<WikiUri> {
    let rest = uri.strip_prefix("wiki://")?;
    if rest.is_empty() || rest.starts_with('/') {
        return None;
    }
    if rest
        .chars()
        .any(|ch| ch.is_whitespace() || matches!(ch, '?' | '#'))
    {
        return None;
    }

    if let Some(uuid) = rest.strip_prefix("new/") {
        if uuid.is_empty() || uuid.contains('/') {
            return None;
        }
        return Some(WikiUri {
            id: rest.to_owned(),
            is_new: true,
        });
    }

    if rest.contains('/') {
        return None;
    }

    Some(WikiUri {
        id: rest.to_owned(),
        is_new: false,
    })
}

/// Render the canonical virtual URI for a Content id.
pub fn wiki_uri(id: &str) -> String {
    format!("wiki://{id}")
}

/// True for a 32-byte Content id rendered as 64 lowercase/uppercase hex digits.
pub fn is_content_id(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "3f2a9c1e00000000000000000000000000000000000000000000000000000001";

    #[test]
    fn parses_content_uri() {
        let parsed = parse_wiki_uri(&format!("wiki://{ID}")).unwrap();
        assert_eq!(parsed.id, ID);
        assert!(!parsed.is_new);
    }

    #[test]
    fn parses_short_fixture_ids() {
        let parsed = parse_wiki_uri("wiki://p-home").unwrap();
        assert_eq!(parsed.id, "p-home");
    }

    #[test]
    fn parses_new_page_uri() {
        let parsed = parse_wiki_uri("wiki://new/6a08f54c-7bd8-4e62-9ee8-1d5f0a9b2c31").unwrap();
        assert_eq!(parsed.id, "new/6a08f54c-7bd8-4e62-9ee8-1d5f0a9b2c31");
        assert!(parsed.is_new);
    }

    #[test]
    fn rejects_non_wiki_and_malformed_uris() {
        for uri in [
            "",
            "file:///tmp/page.md",
            "https://example.com",
            "wiki://",
            "wiki:///p-home",
            "wiki://new/",
            "wiki://p-home/inner",
            "wiki://p-home?x=1",
            "wiki://p home",
        ] {
            assert!(parse_wiki_uri(uri).is_none(), "expected rejection: {uri:?}");
        }
    }

    #[test]
    fn recognizes_content_ids() {
        assert!(is_content_id(ID));
        assert!(!is_content_id("p-home"));
        assert!(!is_content_id(
            "zz2a9c1e00000000000000000000000000000000000000000000000000000001"
        ));
    }
}
