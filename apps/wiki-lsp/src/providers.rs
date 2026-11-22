//! Provider names, presets and kinds surfaced by completion and hover.
//!
//! The v1 provider list is static because `wiki-server` does not expose
//! `interpreter-list` in the M0 line protocol. Keeping it in one place makes
//! it obvious what the LSP promises.

pub struct ProviderInfo {
    pub name: &'static str,
    pub summary: &'static str,
    pub arguments: &'static [&'static str],
}

pub const PROVIDERS: &[ProviderInfo] = &[
    ProviderInfo {
        name: "ledger/balance",
        summary: "Compute ledger balances from the selected snippets.",
        arguments: &[
            "`query(...)` — optional Harmion Query selecting input Contents",
            "`account` — optional account filter",
        ],
    },
    ProviderInfo {
        name: "wiki/backlinks",
        summary: "Return the Contents that link to a Wiki page.",
        arguments: &[
            "`target` — optional Content id or title (defaults to the current page)",
            "`include_broken` — optional bool; include unresolved links",
        ],
    },
    ProviderInfo {
        name: "wiki/outline",
        summary: "Return the headings of a Wiki page.",
        arguments: &["`target` — optional Content id or title (defaults to the current page)"],
    },
    ProviderInfo {
        name: "wiki/transclude",
        summary: "Expand embeds and links into readable text.",
        arguments: &[
            "`target` — Content id or title to expand",
            "`depth` — optional recursion depth, at most 3",
        ],
    },
];

pub fn find(name: &str) -> Option<&'static ProviderInfo> {
    PROVIDERS.iter().find(|provider| provider.name == name)
}

/// Fence presets supported by the authoring UI. These mirror the presets
/// resolved by `wiki-core` and the examples in the requirements.
pub const PRESETS: &[(&str, &str)] = &[
    ("ledger", "language `ledger`; kind `ledger`"),
    ("hledger", "language `ledger`; kinds `ledger`, `free`"),
    ("rust", "language `rust`; no extra kind"),
];

/// Kinds offered in the `preset|kinds` position. Kinds discovered on the
/// server are merged in by completion.
pub const KINDS: &[&str] = &["code", "free", "ledger", "note"];
