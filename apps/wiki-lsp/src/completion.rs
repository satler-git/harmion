//! Context-aware completion for wiki links, interpreter calls and `!` fences.

use std::collections::BTreeSet;

use crate::protocol::{
    COMPLETION_ENUM_MEMBER, COMPLETION_FUNCTION, COMPLETION_KEYWORD, COMPLETION_REFERENCE,
    CompletionItem, Position,
};
use crate::providers;
use crate::server::ContentSummary;
use crate::text;

pub fn complete(text: &str, position: Position, index: &[ContentSummary]) -> Vec<CompletionItem> {
    let byte = text::offset_at(text, position);

    if let Some(link) = text::pending_link_at(text, byte) {
        if link.in_label {
            // The label side is free text.
            return Vec::new();
        }
        return title_items(&link.typed, index);
    }

    if let Some(provider) = text::pending_provider_at(text, byte) {
        return provider_items(&provider.typed);
    }

    if let Some(fence) = text::pending_fence_at(text, byte) {
        return match fence {
            text::PendingFence::Preset { typed } => preset_items(&typed),
            text::PendingFence::Kind { typed, preset } => {
                let _ = preset;
                kind_items(&typed, index)
            }
        };
    }

    Vec::new()
}

fn matches(prefix: &str, value: &str) -> bool {
    prefix.is_empty() || value.to_lowercase().starts_with(&prefix.to_lowercase())
}

fn title_items(typed: &str, index: &[ContentSummary]) -> Vec<CompletionItem> {
    let mut seen = BTreeSet::new();
    let mut items = Vec::new();
    for content in index {
        if !matches(typed, &content.title) || !seen.insert(content.title.clone()) {
            continue;
        }
        let mut detail = content.content_type.clone();
        if let Some(language) = &content.language {
            detail.push_str(&format!(" · {language}"));
        }
        detail.push_str(&format!(" · {}", content.id));
        items.push(CompletionItem {
            label: content.title.clone(),
            kind: COMPLETION_REFERENCE,
            detail,
            insert_text: content.title.clone(),
        });
    }
    items.sort_by(|left, right| left.label.cmp(&right.label));
    items
}

fn provider_items(typed: &str) -> Vec<CompletionItem> {
    providers::PROVIDERS
        .iter()
        .filter(|provider| matches(typed, provider.name))
        .map(|provider| CompletionItem {
            label: provider.name.to_owned(),
            kind: COMPLETION_FUNCTION,
            detail: provider.summary.to_owned(),
            insert_text: provider.name.to_owned(),
        })
        .collect()
}

fn preset_items(typed: &str) -> Vec<CompletionItem> {
    providers::PRESETS
        .iter()
        .filter(|(preset, _)| matches(typed, preset))
        .map(|(preset, detail)| CompletionItem {
            label: (*preset).to_owned(),
            kind: COMPLETION_KEYWORD,
            detail: (*detail).to_owned(),
            insert_text: (*preset).to_owned(),
        })
        .collect()
}

fn kind_items(typed: &str, index: &[ContentSummary]) -> Vec<CompletionItem> {
    let mut kinds = BTreeSet::new();
    for kind in providers::KINDS {
        kinds.insert((*kind).to_owned());
    }
    for content in index {
        for kind in &content.kinds {
            if kind != "wiki" {
                kinds.insert(kind.clone());
            }
        }
    }

    kinds
        .into_iter()
        .filter(|kind| matches(typed, kind))
        .map(|kind| CompletionItem {
            label: kind.clone(),
            kind: COMPLETION_ENUM_MEMBER,
            detail: "Harmion kind".to_owned(),
            insert_text: kind,
        })
        .collect()
}
