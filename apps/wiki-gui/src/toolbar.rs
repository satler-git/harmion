//! Window toolbar: breadcrumb, global search and the editor hand-off.
use super::*;

pub(super) fn toolbar(state: &AppState) -> Div {
    let tab = state.active_tab();
    let title = tab.as_ref().map(|tab| tab.title.clone());
    let editor = state.editor.get();
    let source = state.source.get();
    let fallback = source != Source::Server;

    let mut crumbs = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(7.0_f32)
        .min_w(0.0_f32)
        .child(ui_text("Wiki", 13.0, theme().text_disabled));
    if let Some(title) = &title {
        crumbs = crumbs
            .child(ui_text("/", 12.0, theme().text_disabled))
            .child(
                ui_text(title, 13.5, theme().text)
                    .font_weight(FontWeight::SEMIBOLD)
                    .max_w(360.0_f32),
            );
    }
    if fallback {
        crumbs = crumbs.child(
            div()
                .ml(4.0_f32)
                .px(7.0_f32)
                .py(2.0_f32)
                .rounded(5.0_f32)
                .bg(rgb(0x362b18))
                .flex()
                .flex_row()
                .items_center()
                .gap(5.0_f32)
                .child(dot(rgb(0xe0af68)))
                .child(ui_text(source_label(source), 9.0, rgb(0xe6bd7a))),
        );
    }

    let left = div()
        .flex_1()
        .min_w(0.0_f32)
        .flex()
        .flex_row()
        .items_center()
        .gap(4.0_f32)
        .child(
            icon_button(icons::VIEW_SIDEBAR, "サイドバー Ctrl+\\").on_click({
                let state = state.clone();
                move |_| state.sidebar_open.update(|open| *open = !*open)
            }),
        )
        .child(icon_button(icons::ARROW_BACK, "戻る Ctrl+[").on_click({
            let state = state.clone();
            move |_| state.back()
        }))
        .child(icon_button(icons::ARROW_FORWARD, "進む Ctrl+]").on_click({
            let state = state.clone();
            move |_| state.forward()
        }))
        .child(vdiv())
        .child(crumbs);

    let editor_label = tab
        .as_ref()
        .map(|tab| tab.title.clone())
        .unwrap_or_else(|| "—".to_owned());
    let editor_color = if editor.error.is_some() {
        rgb(0xff6b62)
    } else if editor.attached {
        theme().success
    } else {
        theme().text_disabled
    };

    div()
        .h(56.0_f32)
        .px(16.0_f32)
        .flex()
        .flex_row()
        .items_center()
        .gap(14.0_f32)
        .bg(theme().page)
        .border_b(1.0_f32)
        .border_color(theme().border)
        .child(left)
        .child(
            // A real search field, not a button that merely looks like one: it opens the
            // palette with the caret already in place.
            div()
                .w(320.0_f32)
                .h(32.0_f32)
                .px(11.0_f32)
                .rounded(8.0_f32)
                .bg(theme().surface_muted)
                .border(1.0_f32)
                .border_color(theme().border)
                .flex()
                .flex_row()
                .items_center()
                .gap(8.0_f32)
                .hover(|style| {
                    style
                        .bg(theme().surface)
                        .border_color(theme().border_strong)
                })
                .cursor(CursorIcon::Pointer)
                .on_click({
                    let state = state.clone();
                    move |_| state.open_palette(PaletteMode::Search)
                })
                .access(|access| access.label("検索 Ctrl+K"))
                .child(icon(icons::SEARCH, 16.0).text_color(theme().text_disabled))
                .child(ui_text(
                    "ページ・本文・タグを検索",
                    11.5,
                    theme().text_disabled,
                ))
                .child(div().flex_1())
                .child(kbd("Ctrl+K")),
        )
        .child(
            div()
                .flex_1()
                .flex()
                .flex_row()
                .items_center()
                .justify_end()
                .gap(6.0_f32)
                .child(
                    div()
                        .h(32.0_f32)
                        .px(10.0_f32)
                        .rounded(8.0_f32)
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(7.0_f32)
                        .hover(|style| style.bg(theme().surface_muted))
                        .cursor(CursorIcon::Pointer)
                        .on_click({
                            let state = state.clone();
                            move |_| state.open_editor()
                        })
                        .access(|access| access.label("Neovide で編集 Ctrl+E"))
                        .child(icon(icons::EDIT, 14.0).text_color(editor_color))
                        .child(
                            ui_text("Neovide", 10.5, theme().text_muted)
                                .font_weight(FontWeight::MEDIUM),
                        )
                        .child(ui_text("·", 10.0, theme().text_disabled))
                        .child(
                            ui_text(truncate_chars(&editor_label, 16), 11.0, theme().text)
                                .max_w(130.0_f32),
                        )
                        .child(kbd("Ctrl+E")),
                )
                .child(icon_button(icons::INFO, "ページ情報 Ctrl+I").on_click({
                    let state = state.clone();
                    move |_| {
                        state.outline_open.set(false);
                        state
                            .inspector_visible
                            .update(|visible| *visible = !*visible)
                    }
                }))
                .child(show(
                    {
                        let state = state.clone();
                        move || !outline_rail_fits(&state) || state.outline_collapsed.get()
                    },
                    {
                        let state = state.clone();
                        move || {
                            icon_button(icons::TOC, "目次 Ctrl+3").on_click({
                                let state = state.clone();
                                move |_| {
                                    state.inspector_visible.set(false);
                                    toggle_outline(&state);
                                }
                            })
                        }
                    },
                ))
                .child(
                    icon_button(icons::QUESTION_MARK, "キーボードショートカット ?").on_click({
                        let state = state.clone();
                        move |_| state.shortcuts.set(true)
                    }),
                ),
        )
}
