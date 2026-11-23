//! Shared Apple-like view primitives and typography helpers.
use super::*;

pub(super) fn ui_text(label: impl Into<String>, size: f32, color: Color) -> Text {
    text(label.into())
        .font_family(sans())
        .text_size(size)
        .text_color(color)
}

pub(super) fn mono_text(label: impl Into<String>, size: f32, color: Color) -> Text {
    text(label.into())
        .font_family(mono())
        .text_size(size)
        .text_color(color)
}

pub(super) fn dot(color: Color) -> Div {
    div().w(7.0_f32).h(7.0_f32).rounded(3.5_f32).bg(color)
}

pub(super) fn kbd(label: impl Into<String>) -> Div {
    div()
        .h(16.0_f32)
        .px(4.0_f32)
        .rounded(3.0_f32)
        .bg(theme().surface_muted)
        .flex()
        .items_center()
        .justify_center()
        .child(ui_text(label, 9.0, theme().text_muted))
}

pub(super) fn section_label(label: impl Into<String>) -> Text {
    ui_text(label, 11.0, theme().text_disabled).font_weight(FontWeight::SEMIBOLD)
}

pub(super) fn chip(label: impl Into<String>, color: Color) -> Div {
    div()
        .px(6.0_f32)
        .py(1.0_f32)
        .rounded(4.0_f32)
        .bg(theme().surface_muted)
        .child(mono_text(label, 8.5, color))
}

pub(super) fn vdiv() -> Div {
    div().w(1.0_f32).h(18.0_f32).mx(4.0_f32).bg(theme().border)
}

pub(super) fn icon_button(glyph: &str, tooltip: &str) -> Div {
    div()
        .h(30.0_f32)
        .min_w(30.0_f32)
        .px(8.0_f32)
        .rounded(7.0_f32)
        .flex()
        .items_center()
        .justify_center()
        .hover(|style| style.bg(theme().surface_muted))
        .cursor(CursorIcon::Pointer)
        .child(ui_text(glyph, 14.0, theme().text_muted))
        .access({
            let tooltip = tooltip.to_owned();
            move |access| access.label(tooltip.clone())
        })
}

pub(super) fn nav_row(
    glyph: &str,
    label: &str,
    count: Option<usize>,
    active: bool,
    on_click: impl Fn() + 'static,
) -> Div {
    let mut row = div()
        .h(29.0_f32)
        .px(9.0_f32)
        .rounded(6.0_f32)
        .flex()
        .flex_row()
        .items_center()
        .gap(8.0_f32)
        .hover(|style| style.bg(theme().surface_muted))
        .on_click(move |_| on_click())
        .child(ui_text(
            glyph,
            12.0,
            if active {
                theme().on_accent
            } else {
                theme().text_disabled
            },
        ))
        .child(ui_text(
            label,
            13.0,
            if active {
                theme().on_accent
            } else {
                theme().text
            },
        ));
    if active {
        row = row.bg(theme().accent);
    }
    if let Some(count) = count {
        row = row.child(div().flex_1()).child(ui_text(
            count.to_string(),
            11.0,
            if active {
                rgb(0xffffff)
            } else {
                theme().text_disabled
            },
        ));
    }
    row
}

pub(super) fn divider() -> Div {
    div().my(16.0_f32).h(1.0_f32).bg(theme().border)
}
