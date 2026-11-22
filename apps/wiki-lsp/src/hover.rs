//! Markdown hover bodies for pages, snippets and providers.

use harmion_wiki_core::FenceSpec;

use crate::providers::ProviderInfo;
use crate::server::ContentSummary;

pub fn summary_markdown(summary: &ContentSummary) -> String {
    let mut lines = Vec::new();
    lines.push(format!("### {}", summary.title));
    lines.push(String::new());
    lines.push(format!("- **type** `{}`", summary.content_type));
    lines.push(format!("- **id** `{}`", summary.id));
    if let Some(format) = &summary.format {
        lines.push(format!("- **format** `{format}`"));
    }
    if let Some(language) = &summary.language {
        lines.push(format!("- **language** `{language}`"));
    }
    if !summary.kinds.is_empty() {
        lines.push(format!("- **kinds** {}", kinds_list(&summary.kinds)));
    }
    if !summary.excerpt.is_empty() {
        lines.push(String::new());
        lines.push(format!("> {}", summary.excerpt.replace('\n', " ")));
    }
    lines.join("\n")
}

pub fn fence_markdown(spec: &FenceSpec, summary: Option<&ContentSummary>) -> String {
    let mut lines = Vec::new();
    if let Some(summary) = summary {
        lines.push(format!("### {}", summary.title));
        lines.push(String::new());
        lines.push(format!("- **snippet** `{}`", summary.id));
        if let Some(format) = &summary.format {
            lines.push(format!("- **format** `{format}`"));
        }
        if let Some(language) = &summary.language {
            lines.push(format!("- **language** `{language}`"));
        }
        if !summary.kinds.is_empty() {
            lines.push(format!("- **kinds** {}", kinds_list(&summary.kinds)));
        }
        if !summary.excerpt.is_empty() {
            lines.push(String::new());
            lines.push(format!("> {}", summary.excerpt.replace('\n', " ")));
        }
        lines.push(String::new());
    } else {
        let info = if spec.kinds.is_empty() {
            spec.preset.clone()
        } else {
            format!("{}|{}", spec.preset, spec.kinds.join(","))
        };
        lines.push(format!("### `!```{info}`"));
        lines.push(String::new());
    }

    lines.push(format!("- **preset** `{}`", spec.preset));
    lines.push(format!("- **language** `{}`", spec.language));
    if !spec.kinds.is_empty() {
        lines.push(format!("- **kinds** {}", kinds_list(&spec.kinds)));
    }
    match &spec.id {
        Some(id) => lines.push(format!("- **canonical id** `{id}`")),
        None => lines.push(
            "- Saving this fence creates a snippet and rewrites it to `preset|kinds||id`."
                .to_owned(),
        ),
    }
    lines.join("\n")
}

pub fn provider_markdown(provider: &ProviderInfo) -> String {
    let mut lines = vec![
        format!("### `{}`", provider.name),
        String::new(),
        provider.summary.to_owned(),
        String::new(),
        "**Arguments**".to_owned(),
    ];
    for argument in provider.arguments {
        lines.push(format!("- {argument}"));
    }
    lines.join("\n")
}

fn kinds_list(kinds: &[String]) -> String {
    kinds
        .iter()
        .map(|kind| format!("`{kind}`"))
        .collect::<Vec<_>>()
        .join(", ")
}
