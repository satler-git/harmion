//! Command palette and search overlay.
use super::*;

pub(super) fn palette(state: &AppState) -> Show {
    let show_state = state.clone();
    show(move || show_state.palette.get().is_some(), {
        let state = state.clone();
        move || palette_card(&state)
    })
}

fn palette_card(state: &AppState) -> Div {
    let state_for_close = state.clone();
    div()
        .absolute()
        .top(0.0_f32)
        .left(0.0_f32)
        .right(0.0_f32)
        .bottom(0.0_f32)
        .z_index(40)
        .bg(rgba(0x00000099))
        .flex()
        .items_center()
        .justify_center()
        .on_click(move |_| state_for_close.close_palette())
        .child(palette_card_inner(state).on_click(|event| event.stop_propagation()))
}

fn palette_card_inner(state: &AppState) -> Div {
    let input = node_ref();
    request_frame(move |_| input.focus());
    let mode = state.palette.get().unwrap_or(PaletteMode::Search);
    let results_state = state.clone();
    div()
        .w(560.0_f32)
        .z_index(41)
        .rounded(10.0_f32)
        .bg(theme().surface)
        .border(1.0_f32)
        .border_color(theme().border)
        .shadow_lg()
        .p(8.0_f32)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(7.0_f32)
                .px(8.0_f32)
                .child(ui_text(
                    match mode {
                        PaletteMode::Search => "◎",
                        PaletteMode::Rename | PaletteMode::Tags => "✎",
                        PaletteMode::QuickOpen => "›",
                    },
                    14.0,
                    theme().text_muted,
                ))
                .child(
                    text_input()
                        .value(state.query)
                        .placeholder(match mode {
                            PaletteMode::Search => "検索 — タイトル・本文・タグ",
                            PaletteMode::Rename => "新しいタイトル",
                            PaletteMode::Tags => "タグをカンマ区切りで入力",
                            PaletteMode::QuickOpen => "クイックオープン — ページ名",
                        })
                        .font_family(sans())
                        .text_size(12.5_f32)
                        .text_color(theme().text)
                        .w_full()
                        .h(28.0_f32)
                        .px(6.0_f32)
                        .node_ref(input)
                        .on_input({
                            let state = state.clone();
                            move |text| state.on_query(text)
                        })
                        .on_submit({
                            let state = state.clone();
                            move |_| {
                                let query = state.query.get_untracked();
                                match state.palette.get_untracked() {
                                    Some(PaletteMode::Rename) => state.rename_active(&query),
                                    Some(PaletteMode::Tags) => state.set_tags(&query),
                                    _ => {
                                        let results = state.results.get_untracked();
                                        let index = state
                                            .palette_selected
                                            .get_untracked()
                                            .min(results.len().saturating_sub(1));
                                        if let Some(summary) = results.get(index) {
                                            state.open_summary(summary);
                                        }
                                    }
                                }
                            }
                        })
                        .on_key_down({
                            let state = state.clone();
                            move |event| {
                                if event.key == Key::Named(NamedKey::Escape) {
                                    event.prevent_default();
                                    state.close_palette();
                                    return;
                                }
                                let length = state.results.get_untracked().len();
                                if length == 0 {
                                    return;
                                }
                                if event.key == Key::Named(NamedKey::ArrowDown) {
                                    event.prevent_default();
                                    let index = (state.palette_selected.get_untracked() + 1)
                                        .min(length - 1);
                                    state.palette_selected.set(index);
                                } else if event.key == Key::Named(NamedKey::ArrowUp) {
                                    event.prevent_default();
                                    let index =
                                        state.palette_selected.get_untracked().saturating_sub(1);
                                    state.palette_selected.set(index);
                                }
                            }
                        }),
                )
                .child(kbd("Esc")),
        )
        .child(dyn_view(move || {
            let results = results_state.results.get();
            let selected = results_state.palette_selected.get();
            let error = results_state.palette_error.get();
            let mut list = div().flex().flex_col();
            if let Some(error) = error {
                list = list.child(ui_text(error, 10.5, rgb(0xe0af68)).px(10.0_f32).py(6.0_f32));
            }
            if results.is_empty() {
                list = list.child(
                    ui_text("一致する項目はありません。", 12.0, theme().text_disabled)
                        .px(10.0_f32)
                        .py(8.0_f32),
                );
            }
            for (index, summary) in results.iter().take(20).enumerate() {
                let is_selected = index == selected;
                list = list.child(
                    div()
                        .px(10.0_f32)
                        .py(7.0_f32)
                        .rounded(6.0_f32)
                        .bg(if is_selected {
                            theme().surface_muted
                        } else {
                            Color::TRANSPARENT
                        })
                        .border_l(2.0_f32)
                        .border_color(if is_selected {
                            theme().accent
                        } else {
                            Color::TRANSPARENT
                        })
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(9.0_f32)
                        .hover(|style| style.bg(theme().surface_muted))
                        .cursor(CursorIcon::Pointer)
                        .on_click({
                            let state = results_state.clone();
                            let summary = summary.clone();
                            move |_| state.open_summary(&summary)
                        })
                        .child(
                            ui_text(truncate_chars(&summary.title, 32), 12.5, theme().text)
                                .flex_1(),
                        )
                        .child(mono_text(summary.kind_label(), 9.0, theme().text_disabled))
                        .child(ui_text(&summary.updated_label, 10.0, theme().text_disabled)),
                );
            }
            list
        }))
        .child(
            div()
                .px(10.0_f32)
                .pt(5.0_f32)
                .flex()
                .justify_between()
                .child(ui_text("↑↓ 移動 · Enter 開く", 9.0, theme().text_disabled))
                .child(ui_text("Esc 閉じる", 9.0, theme().text_disabled))
                .child(ui_text(
                    "Ctrl+K 検索 · Ctrl+P クイックオープン",
                    9.0,
                    theme().text_disabled,
                )),
        )
}
