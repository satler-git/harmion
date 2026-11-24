//! Collapsible 2-hop link shelf.
use super::*;

pub(super) fn shelf(state: &AppState) -> Div {
    let pinned = state.shelf_pinned;
    let dismissed = state.shelf_dismissed;
    let sidebar_state = state.clone();
    let count_state = state.clone();
    let body_state = state.clone();
    div()
        .absolute()
        .right(0.0_f32)
        .bottom(28.0_f32)
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
        // The shelf opens only on an explicit click or shortcut. Opening it on hover made the
        // reader jump whenever the pointer drifted near the bottom edge.
        .dyn_style(move |style| {
            let open = pinned.get() && !dismissed.get();
            style
                .left(if sidebar_state.sidebar_open.get() {
                    264.0
                } else {
                    0.0
                })
                .translate_y(if open {
                    0.0
                } else {
                    SHELF_HEIGHT - SHELF_HANDLE
                })
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
                .child(icon(icons::HUB, 15.0).text_color(theme().accent_text))
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
                .child(div().flex_1())
                .child(ui_text(
                    if pinned.get() {
                        "クリックで閉じる ·"
                    } else {
                        "クリックで表示 ·"
                    },
                    9.5,
                    theme().text_disabled,
                ))
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
                        .cursor(CursorIcon::Pointer)
                        .on_click(move |event| {
                            event.stop_propagation();
                            pinned.set(false);
                            dismissed.set(true);
                        })
                        .child(icon(icons::CLOSE, 14.0).text_color(theme().text_muted)),
                )
                .child(dyn_view(move || {
                    let open = pinned.get() && !dismissed.get();
                    icon(
                        if open {
                            icons::EXPAND_LESS
                        } else {
                            icons::EXPAND_MORE
                        },
                        16.0,
                    )
                    .text_color(theme().text_disabled)
                })),
        )
        .child(show(
            move || pinned.get() && !dismissed.get(),
            move || {
                let body_state = body_state.clone();
                div()
                    .h(SHELF_HEIGHT - SHELF_HANDLE)
                    .px(20.0_f32)
                    .pt(8.0_f32)
                    .pb(14.0_f32)
                    .overflow_y_scroll()
                    .child(dyn_view(move || shelf_body(&body_state)))
            },
        ))
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
    let open_state = state.clone();
    let target = hop.target.clone();
    let count = if target.refs > 0 {
        target.refs
    } else {
        group_count
    };
    let kind_tag = target.kind_label();
    let row = div()
        .rounded(6.0_f32)
        .px(8.0_f32)
        .py(7.0_f32)
        .flex()
        .flex_col()
        .hover(|style| style.bg(theme().surface_muted))
        .cursor(CursorIcon::Pointer)
        .on_click(move |_| open_state.open_summary(&target))
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
        );
    context_menu(row, link_menu_entries(state, &hop.target))
}
