//! Window toolbar: navigation, title, search and editor status.
use super::*;

pub(super) fn toolbar(state: &AppState) -> Div {
    let tab = state.active_tab();
    let title = tab
        .as_ref()
        .map(|tab| tab.title.clone())
        .unwrap_or_else(|| "Wiki".to_owned());
    let editor = state.editor.get();
    let fallback = state.source.get() != Source::Server;

    let mut left = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(5.0_f32)
        .child(icon_button("▤", "サイドバー"))
        .child(vdiv())
        .child(icon_button("‹", "戻る Ctrl+[").on_click({
            let state = state.clone();
            move |_| state.back()
        }))
        .child(icon_button("›", "進む Ctrl+]").on_click({
            let state = state.clone();
            move |_| state.forward()
        }))
        .child(vdiv())
        .child(ui_text(&title, 13.0, theme().text).font_weight(FontWeight::SEMIBOLD))
        .child(ui_text("Wiki", 12.0, theme().text_disabled));
    if fallback {
        left = left.child(
            div()
                .px(6.0_f32)
                .py(2.0_f32)
                .rounded(4.0_f32)
                .bg(rgb(0x3a2a12))
                .child(ui_text(
                    source_label(state.source.get()),
                    9.0,
                    rgb(0xe0af68),
                )),
        );
    }

    let editor_label = tab
        .as_ref()
        .map(|tab| tab.title.clone())
        .unwrap_or_else(|| "—".to_owned());
    let editor_color = if editor.error.is_some() {
        rgb(0xff453a)
    } else if editor.attached {
        theme().success
    } else {
        theme().text_disabled
    };

    div()
        .h(52.0_f32)
        .px(14.0_f32)
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .bg(theme().page)
        .border_b(1.0_f32)
        .border_color(theme().border)
        .child(left)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(6.0_f32)
                .child(
                    div()
                        .h(30.0_f32)
                        .px(9.0_f32)
                        .rounded(7.0_f32)
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(6.0_f32)
                        .hover(|style| style.bg(theme().surface_muted))
                        .cursor(CursorIcon::Pointer)
                        .on_click({
                            let state = state.clone();
                            move |_| state.open_palette(PaletteMode::Search)
                        })
                        .access(|access| access.label("検索 Ctrl+K"))
                        .child(ui_text("検索", 11.5, theme().text_muted))
                        .child(kbd("Ctrl+K")),
                )
                .child(
                    div()
                        .h(30.0_f32)
                        .px(9.0_f32)
                        .rounded(7.0_f32)
                        .bg(theme().surface_muted)
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(6.0_f32)
                        .hover(|style| style.bg(theme().hover))
                        .on_click({
                            let state = state.clone();
                            move |_| state.open_editor()
                        })
                        .child(dot(editor_color))
                        .child(
                            ui_text("Neovide", 11.0, theme().text_muted)
                                .font_weight(FontWeight::MEDIUM),
                        )
                        .child(ui_text("·", 10.0, theme().text_disabled))
                        .child(ui_text(&editor_label, 11.0, theme().text))
                        .child(kbd("Ctrl+E")),
                )
                .child(
                    div()
                        .h(30.0_f32)
                        .px(7.0_f32)
                        .rounded(7.0_f32)
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(4.0_f32)
                        .hover(|style| style.bg(theme().hover))
                        .on_click({
                            let state = state.clone();
                            move |_| {
                                state
                                    .inspector_visible
                                    .update(|visible| *visible = !*visible)
                            }
                        })
                        .child(ui_text("ⓘ", 14.0, theme().text_muted))
                        .child(kbd("Ctrl+I")),
                ),
        )
}
