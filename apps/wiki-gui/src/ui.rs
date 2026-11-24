//! Shared view primitives, iconography and typography helpers.
use super::*;

use anti_webui_symbols_outlined::Symbol;
use std::cell::RefCell;
use std::collections::HashSet;

thread_local! {
    static ICON_FONTS: RefCell<HashSet<*const u8>> = RefCell::new(HashSet::new());
}

/// Material Symbols are stored one small font per glyph. Each font belongs to the thread that
/// uses it and is registered once, however many times the icon appears.
pub(super) fn icon(symbol: &'static Symbol, size: f32) -> Div {
    if ICON_FONTS.with(|fonts| fonts.borrow_mut().insert(symbol.font.as_ptr())) {
        register_font_as(symbol.font, symbol.family).expect("the font of an icon");
    }
    div()
        .size(size)
        .flex_none()
        .font_family(symbol.family)
        .text_size(size)
        .line_height(LineHeight::Relative(1.0))
        .font_weight(FontWeight::NORMAL)
        .letter_spacing(0.0_f32)
        .text_align(TextAlign::Center)
        .select_none()
        .child(text(symbol.ch.to_string()))
}

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
        .child(ui_text(label, 10.0, theme().text_muted))
}

pub(super) fn section_label(label: impl Into<String>) -> Text {
    ui_text(label, 11.0, theme().text_muted).font_weight(FontWeight::SEMIBOLD)
}

pub(super) fn chip(label: impl Into<String>, color: Color) -> Div {
    div()
        .px(6.0_f32)
        .py(1.0_f32)
        .rounded(4.0_f32)
        .bg(theme().surface_muted)
        .child(mono_text(label, 9.5, color))
}

pub(super) fn vdiv() -> Div {
    div().w(1.0_f32).h(18.0_f32).mx(4.0_f32).bg(theme().border)
}

pub(super) fn icon_button(symbol: &'static Symbol, label: &str) -> Div {
    let target = div()
        .h(30.0_f32)
        .min_w(30.0_f32)
        .px(7.0_f32)
        .rounded(7.0_f32)
        .flex()
        .items_center()
        .justify_center()
        .hover(|style| style.bg(theme().surface_muted))
        .cursor(CursorIcon::Pointer)
        .child(icon(symbol, 17.0))
        .access({
            let label = label.to_owned();
            move |access| access.label(label.clone())
        });
    tooltip(target, label)
}

/// A quiet icon+label action used inside panels and the toolbar.
pub(super) fn icon_text_button(
    symbol: &'static Symbol,
    label: &str,
    font_size: f32,
    color: Color,
) -> Div {
    div()
        .h(28.0_f32)
        .px(7.0_f32)
        .rounded(6.0_f32)
        .flex()
        .flex_row()
        .items_center()
        .gap(6.0_f32)
        .hover(|style| style.bg(theme().surface_muted))
        .cursor(CursorIcon::Pointer)
        .child(icon(symbol, 15.0))
        .child(ui_text(label, font_size, color))
}

pub(super) fn nav_row(
    symbol: &'static Symbol,
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
        .child(icon(symbol, 17.0).text_color(if active {
            theme().accent
        } else {
            theme().text_disabled
        }))
        .child(ui_text(label, 13.0, theme().text));
    if active {
        row = row
            .bg(theme().accent_muted)
            .border_l(2.0_f32)
            .border_color(theme().accent)
            .pl(7.0_f32);
    }
    if let Some(count) = count {
        row = row.child(div().flex_1()).child(ui_text(
            count.to_string(),
            11.0,
            if active {
                theme().accent_text
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
