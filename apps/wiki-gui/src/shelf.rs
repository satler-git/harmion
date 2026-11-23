//! Collapsible 2-hop link shelf.
use super::*;

pub(super) fn shelf(state: &AppState) -> Div {
    let pinned = state.shelf_pinned;
    let hovered = state.shelf_hover;
    let dismissed = state.shelf_dismissed;
    let count_state = state.clone();
    let body_state = state.clone();
    div()
        .absolute()
        .left(236.0_f32)
        .right(0.0_f32)
        .bottom(0.0_f32)
        .h(SHELF_HEIGHT)
        .z_index(20)
        .bg(theme().surface)
        .border_t(1.0_f32)
        .border_color(theme().border)
        .shadow(BoxShadow {
            offset_x: 0.0,
            offset_y: -16.0,
            blur: 40.0,
            spread: 0.0,
            color: rgba(0x00000057),
            inset: false,
        })
        .transition(Transition::ms(240).cubic_bezier(0.2, 0.8, 0.2, 1.0))
        .dyn_style(move |style| {
            let open = (pinned.get() || hovered.get()) && !dismissed.get();
            style.translate_y(if open {
                0.0
            } else {
                SHELF_HEIGHT - SHELF_HANDLE
            })
        })
        .on_hover(move |over| {
            hovered.set(over);
            if !over {
                dismissed.set(false);
            }
        })
        .child(
            div()
                .h(SHELF_HANDLE)
                .px(20.0_f32)
                .flex()
                .flex_row()
                .items_center()
                .gap(9.0_f32)
                .on_click(move |_| {
                    dismissed.set(false);
                    pinned.update(|pinned| *pinned = !*pinned);
                })
                .child(
                    ui_text("2-hop リンク", 11.0, theme().text).font_weight(FontWeight::SEMIBOLD),
                )
                .child(dyn_view(move || {
                    let total = count_state
                        .active_workspace()
                        .map(|workspace| workspace.links.two_hop.len())
                        .unwrap_or(0);
                    div()
                        .min_w(17.0_f32)
                        .h(16.0_f32)
                        .px(4.0_f32)
                        .rounded(4.0_f32)
                        .bg(theme().surface_muted)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(ui_text(total.to_string(), 9.0, theme().text_muted))
                }))
                .child(ui_text("すべて見る", 11.0, theme().text_muted))
                .child(div().flex_1())
                .child(ui_text("ホバーで表示 ·", 9.5, theme().text_disabled))
                .child(kbd("Ctrl+Alt+L"))
                .child(
                    div()
                        .w(22.0_f32)
                        .h(22.0_f32)
                        .rounded(5.0_f32)
                        .flex()
                        .items_center()
                        .justify_center()
                        .hover(|style| style.bg(theme().surface_muted))
                        .on_click(move |event| {
                            event.stop_propagation();
                            pinned.set(false);
                            dismissed.set(true);
                        })
                        .child(ui_text("×", 12.0, theme().text_muted)),
                )
                .child(dyn_view(move || {
                    let open = (pinned.get() || hovered.get()) && !dismissed.get();
                    ui_text(if open { "▾" } else { "▴" }, 11.0, theme().text_disabled)
                })),
        )
        .child(
            div()
                .h(SHELF_HEIGHT - SHELF_HANDLE)
                .px(20.0_f32)
                .pt(8.0_f32)
                .pb(14.0_f32)
                .overflow_y_scroll()
                .child(dyn_view(move || shelf_body(&body_state))),
        )
}

pub(super) fn shelf_body(state: &AppState) -> Div {
    let workspace = state.active_workspace();
    let groups = workspace.as_ref().map(ordered_groups).unwrap_or_default();
    if groups.is_empty() {
        return div().child(ui_text(
            "このページから 2-hop で辿れるリンクはありません。",
            12.0,
            theme().text_disabled,
        ));
    }
    let mut columns = div().flex().flex_col().gap(16.0_f32);
    for chunk in groups.chunks(3) {
        let mut row = div().flex().flex_row().gap(24.0_f32);
        for (via, items) in chunk {
            let mut group = div().flex_1().min_w(0.0_f32).flex().flex_col().child(
                div()
                    .mb(3.0_f32)
                    .px(7.0_f32)
                    .flex()
                    .justify_between()
                    .child(ui_text(
                        format!("{} から", via.title),
                        10.0,
                        theme().text_muted,
                    ))
                    .child(ui_text(
                        items.len().to_string(),
                        10.0,
                        theme().text_disabled,
                    )),
            );
            for hop in items {
                group = group.child(hop_row(state, hop, items.len()));
            }
            row = row.child(group);
        }
        columns = columns.child(row);
    }
    columns
}

fn hop_row(state: &AppState, hop: &TwoHop, group_count: usize) -> Div {
    let state = state.clone();
    let target = hop.target.clone();
    let count = if target.refs > 0 {
        target.refs
    } else {
        group_count
    };
    let kind_tag = target.kind_label();
    div()
        .rounded(6.0_f32)
        .px(8.0_f32)
        .py(7.0_f32)
        .flex()
        .flex_col()
        .hover(|style| style.bg(theme().surface_muted))
        .on_click(move |_| state.open_summary(&target))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(6.0_f32)
                .child(
                    ui_text(truncate_chars(&hop.target.title, 22), 11.5, theme().text)
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(mono_text(kind_tag, 8.5, theme().text_disabled)),
        )
        .child(
            ui_text(
                truncate_chars(&hop.target.excerpt, 78),
                10.0,
                theme().text_muted,
            )
            .mt(2.0_f32)
            .line_height(LineHeight::Relative(1.4_f32)),
        )
        .child(
            ui_text(
                format!(
                    "更新 {} · {} リンク · {}",
                    hop.target.updated_label,
                    count,
                    hop.target.kind_label()
                ),
                9.0,
                theme().text_disabled,
            )
            .mt(3.0_f32),
        )
}

// ---------------------------------------------------------------------------
// Palette (non-modal: no backdrop)
// ---------------------------------------------------------------------------

fn palette(state: &AppState) -> Show {
    let show_state = state.clone();
    show(move || show_state.palette.get().is_some(), {
        let state = state.clone();
        move || palette_card(&state)
    })
}

fn palette_card(state: &AppState) -> Div {
    let input = node_ref();
    request_frame(move |_| input.focus());
    let mode = state.palette.get().unwrap_or(PaletteMode::Search);
    let header_state = state.clone();
    let results_state = state.clone();
    div()
        .absolute()
        .top(58.0_f32)
        .left(320.0_f32)
        .w(620.0_f32)
        .z_index(40)
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
                                        if let Some(first) =
                                            state.results.get_untracked().into_iter().next()
                                        {
                                            state.open_summary(&first);
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
                                }
                            }
                        }),
                )
                .child(kbd("Esc")),
        )
        .child(
            div()
                .px(10.0_f32)
                .pt(6.0_f32)
                .pb(4.0_f32)
                .child(dyn_view(move || {
                    let mode = header_state.palette.get().unwrap_or(PaletteMode::Search);
                    let label = match mode {
                        PaletteMode::Search => "検索",
                        PaletteMode::Rename => "タイトルを変更",
                        PaletteMode::Tags => "タグを変更",
                        PaletteMode::QuickOpen => "クイックオープン",
                    };
                    ui_text(label, 9.5, theme().text_disabled)
                })),
        )
        .child(dyn_view(move || {
            let results = results_state.results.get();
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
            for summary in results.iter().take(20) {
                list = list.child(
                    div()
                        .px(10.0_f32)
                        .py(7.0_f32)
                        .rounded(6.0_f32)
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(9.0_f32)
                        .hover(|style| style.bg(theme().surface_muted))
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
                .child(ui_text("Enter 開く", 9.0, theme().text_disabled))
                .child(ui_text("Esc 閉じる", 9.0, theme().text_disabled))
                .child(ui_text(
                    "Ctrl+K 検索 · Ctrl+P クイックオープン",
                    9.0,
                    theme().text_disabled,
                )),
        )
}
