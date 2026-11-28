//! Mote desktop HUD. P0 spike: a layer surface, keyboard focus, IME and a resident
//! process that is summoned through a Unix socket.

mod session;

use anti_webui::core::geometry::RectI;
use anti_webui::prelude::*;
use anti_webui::shell::{Shell, prelude::*};
use anti_webui_symbols_outlined as icons;
use mote::now_micros;
use mote::spool::{PendingCapture, Spool};
use mote_core::Capture;
use serde_json::json;
use session::{Model, Session, Status};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::time::{Duration, Instant};
use time::{OffsetDateTime, UtcOffset, Weekday};

// ---------------------------------------------------------------------------
// Design tokens (OpenPencil "mote-desktop", v2 Quick Panel)
// ---------------------------------------------------------------------------

const PANEL_W: f32 = 640.0;
const PANEL_H: f32 = 680.0;
const DETAIL_W: f32 = PANEL_W;
const DETAIL_H: f32 = PANEL_H;
const CAPTURE_W: f32 = 600.0;
const CAPTURE_H: f32 = 420.0;
/// Room for the drop shadow. The surface is transparent here and takes no input.
const SHADOW_PAD: f32 = 96.0;
const SURFACE_W: i32 = (PANEL_W + SHADOW_PAD * 2.0) as i32;
const SURFACE_H: i32 = (PANEL_H + SHADOW_PAD * 2.0) as i32;

const PANEL: u32 = 0x17141f;
const RAISED: u32 = 0x1d1927;
const ACTION_BAR: u32 = 0x191520;
const STATUS_BAR: u32 = 0x14111b;
const SELECTED: u32 = 0x2a2338;
const BORDER: u32 = 0x2a2436;
const TEXT: u32 = 0xfaf6ff;
const TEXT_SECONDARY: u32 = 0xc9bedb;
const TEXT_MUTED: u32 = 0x8f84a1;
const ACCENT: u32 = 0xd9c0ff;
const ON_ACCENT: u32 = 0x241a33;
const WARNING: u32 = 0xffcf70;
const SUCCESS: u32 = 0x5be38a;

const SANS: &str = "IBM Plex Sans JP";
const MONO: &str = "JetBrains Mono";

fn icon(symbol: &'static icons::Symbol, size: f32) -> Div {
    use std::collections::HashSet;
    thread_local! {
        static FONTS: RefCell<HashSet<*const u8>> = RefCell::new(HashSet::new());
    }
    if FONTS.with(|fonts| fonts.borrow_mut().insert(symbol.font.as_ptr())) {
        register_font_as(symbol.font, symbol.family).expect("the font of an icon");
    }
    div()
        .size(size)
        .flex_none()
        .font_family(symbol.family)
        .text_size(size)
        .line_height(LineHeight::Relative(1.0))
        .text_align(TextAlign::Center)
        .select_none()
        .child(text(symbol.ch.to_string()))
}

fn ui_text(label: impl Into<String>, size: f32, color: u32) -> Text {
    text(label.into())
        .font_family(SANS)
        .text_size(size)
        .text_color(rgb(color))
}

fn mono_text(label: impl Into<String>, size: f32, color: u32) -> Text {
    text(label.into())
        .font_family(MONO)
        .text_size(size)
        .text_color(rgb(color))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    List,
    Capture,
}

impl Mode {
    fn size(self) -> (i32, i32) {
        (SURFACE_W, SURFACE_H)
    }

    fn input(self) -> Vec<RectI> {
        let (w, h) = match self {
            Mode::List => (PANEL_W, PANEL_H),
            Mode::Capture => (CAPTURE_W, CAPTURE_H),
        };
        vec![RectI::new(
            ((SURFACE_W as f32 - w) / 2.0) as i32,
            ((SURFACE_H as f32 - h) / 2.0) as i32,
            w as i32,
            h as i32,
        )]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FilterMode {
    Inbox,
    All,
    Pinned,
    Archived,
}

impl FilterMode {
    fn label(self) -> &'static str {
        match self {
            FilterMode::Inbox => "Inbox",
            FilterMode::All => "All",
            FilterMode::Pinned => "Pinned",
            FilterMode::Archived => "Archived",
        }
    }

    fn next(self) -> Self {
        match self {
            FilterMode::Inbox => FilterMode::All,
            FilterMode::All => FilterMode::Pinned,
            FilterMode::Pinned => FilterMode::Archived,
            FilterMode::Archived => FilterMode::Inbox,
        }
    }

    fn previous(self) -> Self {
        match self {
            FilterMode::Inbox => FilterMode::Archived,
            FilterMode::All => FilterMode::Inbox,
            FilterMode::Pinned => FilterMode::All,
            FilterMode::Archived => FilterMode::Pinned,
        }
    }

    fn matches(self, capture: &Capture) -> bool {
        match self {
            FilterMode::Inbox => !capture.archived,
            FilterMode::All => true,
            FilterMode::Pinned => capture.pinned && !capture.archived,
            FilterMode::Archived => capture.archived,
        }
    }
}

fn filter_count(model: &Model, filter: FilterMode) -> usize {
    let captures = model
        .captures
        .iter()
        .filter(|capture| filter.matches(capture))
        .count();
    if matches!(filter, FilterMode::Inbox | FilterMode::All) {
        captures + model.pending.len()
    } else {
        captures
    }
}

#[derive(Clone, Copy)]
struct UiState {
    query: Signal<String>,
    selected: Signal<usize>,
    selected_id: Signal<Option<String>>,
    search: NodeRef,
    search_focused: Signal<bool>,
    root: NodeRef,
    list: NodeRef,
    detail: Signal<Option<String>>,
    command: Signal<bool>,
    command_query: Signal<String>,
    command_selected: Signal<usize>,
    command_search: NodeRef,
    confirm: Signal<Option<String>>,
    confirm_delete: Signal<bool>,
    reply: Signal<Option<String>>,
    capture_open: Signal<bool>,
    help: Signal<bool>,
    filter: Signal<FilterMode>,
    process: Signal<bool>,
    process_selected: Signal<usize>,
    scroll_y: Signal<f32>,
    detail_scroll: NodeRef,
    last_seen: i64,
}

impl UiState {
    fn new(last_seen: i64) -> Self {
        Self {
            query: signal(String::new()),
            selected: signal(0usize),
            selected_id: signal(None),
            search: node_ref(),
            search_focused: signal(false),
            root: node_ref(),
            list: node_ref(),
            detail: signal(None),
            command: signal(false),
            command_query: signal(String::new()),
            command_selected: signal(0usize),
            command_search: node_ref(),
            confirm: signal(None),
            confirm_delete: signal(false),
            reply: signal(None),
            capture_open: signal(false),
            help: signal(false),
            filter: signal(FilterMode::Inbox),
            process: signal(false),
            process_selected: signal(0usize),
            scroll_y: signal(0.0f32),
            detail_scroll: node_ref(),
            last_seen,
        }
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(index) = args.iter().position(|arg| arg == "--snapshot") {
        let out = args
            .get(index + 1)
            .filter(|value| !value.starts_with("--"))
            .cloned()
            .unwrap_or_else(|| "target/mote-hud-panel.png".into());
        snapshot(&out);
        return;
    }
    if let Some(index) = args.iter().position(|arg| arg == "--bench-list") {
        let count = args
            .get(index + 1)
            .and_then(|value| value.parse().ok())
            .unwrap_or(1000);
        bench_list(count);
        return;
    }
    if args.iter().any(|arg| arg == "--daemon") {
        daemon();
        return;
    }
    let command = match args.first().map(String::as_str) {
        Some("capture") => "capture",
        Some("open") => "open",
        Some("close") => "close",
        Some("quit") => "quit",
        Some("toggle") | None => "toggle",
        Some(other) => {
            eprintln!("mote-hud: unknown command `{other}`");
            std::process::exit(2);
        }
    };
    if !connect_or_start() {
        std::process::exit(1);
    }
    send(command);
}

fn snapshot(out: &str) {
    let scale = 1.5;
    let width = (SURFACE_W as f64 * scale) as u32;
    let height = (SURFACE_H as f64 * scale) as u32;
    anti_webui::app::headless::render_png(out, width, height, scale, || {
        surface(static_panel(&fixture_model(), UiState::new(0)))
    })
    .expect("the snapshot");
    println!("wrote {out} ({width}x{height})");
}

// ---------------------------------------------------------------------------
// IPC
// ---------------------------------------------------------------------------

fn socket_path() -> PathBuf {
    if let Some(path) = std::env::var_os("MOTE_HUD_SOCKET") {
        return PathBuf::from(path);
    }
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    dir.join("mote-hud.sock")
}

fn connect_or_start() -> bool {
    let path = socket_path();
    if UnixStream::connect(&path).is_ok() {
        return true;
    }
    let exe = std::env::current_exe().expect("the current binary");
    let _ = Command::new(exe)
        .arg("--daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(2) {
        if UnixStream::connect(&path).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    eprintln!("mote-hud: the daemon did not start");
    false
}

fn send(command: &str) {
    let path = socket_path();
    match UnixStream::connect(&path) {
        Ok(mut stream) => {
            let _ = stream.write_all(command.as_bytes());
            let _ = stream.write_all(b"\n");
        }
        Err(error) => eprintln!("mote-hud: cannot reach the daemon: {error}"),
    }
}

#[derive(Clone, Copy, Debug)]
enum Message {
    Toggle,
    Open,
    Capture,
    Close,
    Quit,
}

// ---------------------------------------------------------------------------
// Daemon and surface
// ---------------------------------------------------------------------------

fn daemon() {
    let path = socket_path();
    if UnixStream::connect(&path).is_ok() {
        eprintln!("mote-hud: already running");
        return;
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).unwrap_or_else(|error| {
        eprintln!("mote-hud: cannot bind {}: {error}", path.display());
        std::process::exit(1);
    });
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));

    let session = Session::start(None);
    Shell::new()
        .namespace("mote-hud")
        .run(move || {
            let layer: Rc<RefCell<Option<LayerHandle>>> = Rc::new(RefCell::new(None));
            let layer_for_channel = layer.clone();
            let session = session.clone();
            let sender = ui_channel(move |message: Message| {
                let open = layer_for_channel
                    .borrow()
                    .as_ref()
                    .is_some_and(|handle| handle.is_open());
                if open {
                    if let Some(handle) = layer_for_channel.borrow().as_ref() {
                        handle.close();
                    }
                    if matches!(message, Message::Toggle | Message::Close | Message::Quit) {
                        if matches!(message, Message::Quit) {
                            quit();
                        }
                        return;
                    }
                }
                if matches!(message, Message::Quit) {
                    quit();
                    return;
                }
                let mode = match message {
                    Message::Capture => Mode::Capture,
                    Message::Toggle | Message::Open | Message::Close | Message::Quit => Mode::List,
                };
                session.refresh();
                let view_session = session.clone();
                let view_layer = layer.clone();
                let last_seen = mark_last_seen();
                let handle = open_layer(options(mode), move || {
                    layer_view(mode, view_session, UiState::new(last_seen), view_layer)
                });
                *layer_for_channel.borrow_mut() = Some(handle);
            });
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { continue };
                    let mut line = String::new();
                    let _ = BufReader::new(&mut stream).read_line(&mut line);
                    let message = match line.trim() {
                        "capture" => Some(Message::Capture),
                        "open" => Some(Message::Open),
                        "close" => Some(Message::Close),
                        "quit" => Some(Message::Quit),
                        "toggle" => Some(Message::Toggle),
                        _ => None,
                    };
                    if let Some(message) = message {
                        sender.send(message);
                    }
                }
            });
        })
        .expect("the HUD shell");
}

fn options(mode: Mode) -> LayerOptions {
    let (w, h) = mode.size();
    LayerOptions::new()
        .layer(Layer::Overlay)
        .anchor(Anchor::NONE)
        .size(w, h)
        .keyboard(Keyboard::Exclusive)
        .exclusive(Exclusive::Ignore)
        .input_region(InputRegion::Rects(mode.input()))
        .namespace("mote-hud")
}

fn layer_view(
    mode: Mode,
    session: Session,
    ui: UiState,
    layer: Rc<RefCell<Option<LayerHandle>>>,
) -> Div {
    let root = ui.root;
    request_frame(move |_| root.focus());
    surface(match mode {
        Mode::List => live_surface(&session, ui, layer.clone()),
        Mode::Capture => capture_panel(&session),
    })
    .node_ref(root)
    .focusable()
    .on_key_down(move |event| {
        if matches!(mode, Mode::Capture) {
            if event.key == Key::Named(NamedKey::Escape) {
                close_window();
            }
            return;
        }
        if ui.confirm.get().is_some() {
            match &event.key {
                Key::Named(NamedKey::Escape) => {
                    ui.confirm.set(None);
                    ui.confirm_delete.set(false);
                }
                Key::Named(NamedKey::Tab)
                | Key::Named(NamedKey::ArrowLeft)
                | Key::Named(NamedKey::ArrowRight) => {
                    ui.confirm_delete.set(!ui.confirm_delete.get());
                }
                Key::Named(NamedKey::Enter) => {
                    if ui.confirm_delete.get() {
                        if let Some(id) = ui.confirm.get() {
                            session.delete(&id);
                            ui.detail.set(None);
                        }
                    }
                    ui.confirm.set(None);
                    ui.confirm_delete.set(false);
                }
                _ => {}
            }
            return;
        }
        if ui.process.get() {
            match &event.key {
                Key::Named(NamedKey::Escape) => {
                    ui.process.set(false);
                    ui.root.focus();
                }
                Key::Named(NamedKey::ArrowDown) => {
                    let next = (ui.process_selected.get() + 1).min(3);
                    ui.process_selected.set(next);
                }
                Key::Named(NamedKey::ArrowUp) => {
                    let next = ui.process_selected.get().saturating_sub(1);
                    ui.process_selected.set(next);
                }
                Key::Named(NamedKey::Enter) => {
                    let action = ProcessAction::ALL[ui.process_selected.get().min(3)];
                    ui.process.set(false);
                    run_process(action, &session, ui, &layer);
                    ui.root.focus();
                }
                Key::Character(text) => {
                    let action = match text.as_str() {
                        "a" => Some(ProcessAction::Complete),
                        "c" => Some(ProcessAction::Copy),
                        "r" => Some(ProcessAction::Reply),
                        _ => None,
                    };
                    if let Some(action) = action {
                        ui.process.set(false);
                        run_process(action, &session, ui, &layer);
                        ui.root.focus();
                    }
                }
                _ => {}
            }
            return;
        }
        if ui.help.get() {
            if event.key == Key::Named(NamedKey::Escape)
                || matches!(&event.key, Key::Character(text) if text == "?")
            {
                ui.help.set(false);
                root.focus();
            }
            return;
        }
        if ui.capture_open.get() {
            if event.key == Key::Named(NamedKey::Escape) {
                ui.capture_open.set(false);
                root.focus();
            }
            return;
        }
        if ui.reply.get().is_some() {
            if event.key == Key::Named(NamedKey::Escape) {
                ui.reply.set(None);
                root.focus();
            }
            return;
        }
        if event.modifiers.ctrl
            && matches!(&event.key, Key::Character(text) if text.eq_ignore_ascii_case("k"))
        {
            let opening = !ui.command.get();
            ui.command.set(opening);
            if opening {
                ui.command_query.set(String::new());
                ui.command_selected.set(0);
                let search = ui.command_search;
                request_frame(move |_| search.focus());
            } else {
                root.focus();
            }
            return;
        }
        if ui.command.get() {
            match &event.key {
                Key::Named(NamedKey::Escape) => {
                    ui.command.set(false);
                    root.focus();
                }
                Key::Named(NamedKey::ArrowDown) => move_command(session.clone(), ui, 1),
                Key::Named(NamedKey::ArrowUp) => move_command(session.clone(), ui, -1),
                Key::Named(NamedKey::Enter) => {
                    if let Some(action) = command_at(&session, ui) {
                        ui.command.set(false);
                        run_command(action, &session, ui, &layer);
                        ui.root.focus();
                    }
                }
                _ => {}
            }
            return;
        }
        if ui.search_focused.get() {
            match &event.key {
                Key::Named(NamedKey::Escape) => {
                    ui.query.set(String::new());
                    ui.search_focused.set(false);
                    reset_scroll(ui);
                    root.focus();
                }
                Key::Named(NamedKey::Tab) | Key::Named(NamedKey::ArrowDown) => {
                    ui.search_focused.set(false);
                    root.focus();
                }
                Key::Named(NamedKey::Enter) => {
                    ui.search_focused.set(false);
                    root.focus();
                    if ui.detail.get().is_none()
                        && let Some(id) = selected_capture(&session, ui)
                    {
                        ui.detail.set(Some(id));
                    }
                }
                _ => {}
            }
            return;
        }
        match &event.key {
            Key::Named(NamedKey::Escape) => {
                if ui.detail.get().is_some() {
                    ui.detail.set(None);
                    set_surface(&layer, false);
                } else if !ui.query.get().is_empty() {
                    ui.query.set(String::new());
                    reset_scroll(ui);
                } else {
                    close_window();
                }
            }
            Key::Named(NamedKey::Enter) => {
                if ui.detail.get().is_none()
                    && let Some(id) = selected_capture(&session, ui)
                {
                    ui.detail.set(Some(id));
                    set_surface(&layer, true);
                }
            }
            Key::Named(NamedKey::ArrowDown) => {
                if ui.detail.get().is_some() {
                    scroll_detail(ui, 60.0);
                } else {
                    move_selection(&session, ui, 1);
                }
            }
            Key::Named(NamedKey::ArrowUp) => {
                if ui.detail.get().is_some() {
                    scroll_detail(ui, -60.0);
                } else {
                    move_selection(&session, ui, -1);
                }
            }
            Key::Named(NamedKey::PageDown) => {
                if ui.detail.get().is_some() {
                    scroll_detail(ui, 400.0);
                }
            }
            Key::Named(NamedKey::PageUp) => {
                if ui.detail.get().is_some() {
                    scroll_detail(ui, -400.0);
                }
            }
            Key::Named(NamedKey::Space) => {
                if ui.detail.get().is_some() {
                    scroll_detail(ui, 400.0);
                }
            }
            Key::Character(text)
                if (text == "J" || (text.eq_ignore_ascii_case("j") && event.modifiers.shift)) =>
            {
                if ui.detail.get().is_some() {
                    scroll_detail(ui, 300.0);
                } else {
                    move_selection(&session, ui, 5);
                }
            }
            Key::Character(text)
                if (text == "K" || (text.eq_ignore_ascii_case("k") && event.modifiers.shift)) =>
            {
                if ui.detail.get().is_some() {
                    scroll_detail(ui, -300.0);
                } else {
                    move_selection(&session, ui, -5);
                }
            }
            Key::Character(text) if text == "j" => {
                if ui.detail.get().is_some() {
                    scroll_detail(ui, 60.0);
                } else {
                    move_selection(&session, ui, 1);
                }
            }
            Key::Character(text) if text == "k" => {
                if ui.detail.get().is_some() {
                    scroll_detail(ui, -60.0);
                } else {
                    move_selection(&session, ui, -1);
                }
            }
            Key::Character(text)
                if (text == "L" || (text.eq_ignore_ascii_case("l") && event.modifiers.shift))
                    && ui.detail.get().is_none() =>
            {
                ui.filter.set(ui.filter.get().next());
                ui.selected.set(0);
                ui.selected_id.set(None);
                reset_scroll(ui);
            }
            Key::Character(text)
                if (text == "H" || (text.eq_ignore_ascii_case("h") && event.modifiers.shift))
                    && ui.detail.get().is_none() =>
            {
                ui.filter.set(ui.filter.get().previous());
                ui.selected.set(0);
                ui.selected_id.set(None);
                reset_scroll(ui);
            }
            Key::Character(text) if text == "l" && !event.modifiers.shift => {
                if ui.detail.get().is_none()
                    && let Some(id) = selected_capture(&session, ui)
                {
                    ui.detail.set(Some(id));
                }
            }
            Key::Character(text) if text == "h" && !event.modifiers.shift => {
                if ui.detail.get().is_some() {
                    ui.detail.set(None);
                }
            }
            Key::Character(text) if text == "u" => {
                if let Some(id) = current_capture_id(&session, ui)
                    && session
                        .model()
                        .get()
                        .captures
                        .iter()
                        .find(|capture| capture.id == id)
                        .is_some_and(|capture| capture.archived)
                {
                    session.unarchive(&id);
                }
            }
            Key::Character(text) if text == "a" => {
                if let Some(id) = current_capture_id(&session, ui) {
                    let archived = session
                        .model()
                        .get()
                        .captures
                        .iter()
                        .find(|capture| capture.id == id)
                        .is_some_and(|capture| capture.archived);
                    if archived {
                        session.unarchive(&id);
                    } else {
                        session.archive(&id);
                    }
                    if ui.detail.get().is_some() {
                        ui.detail.set(None);
                        set_surface(&layer, false);
                    }
                }
            }
            Key::Character(text) if text == "p" => {
                if let Some(id) = current_capture_id(&session, ui)
                    && let Some(capture) = session
                        .model()
                        .get()
                        .captures
                        .iter()
                        .find(|capture| capture.id == id)
                {
                    session.pin(&id, !capture.pinned);
                }
            }
            Key::Character(text) if text == "R" => {
                session.retry();
            }
            Key::Character(text) if text == "r" => {
                if let Some(id) = current_capture_id(&session, ui) {
                    ui.reply.set(Some(id));
                }
            }
            Key::Character(text) if text == "d" => {
                if let Some(id) = current_capture_id(&session, ui) {
                    ui.confirm_delete.set(false);
                    ui.confirm.set(Some(id));
                }
            }
            Key::Character(text) if text == "c" => {
                if let Some(id) = current_capture_id(&session, ui)
                    && let Some(capture) = session
                        .model()
                        .get()
                        .captures
                        .iter()
                        .find(|capture| capture.id == id)
                {
                    copy_text(copy_body(capture));
                }
            }
            Key::Character(text) if text == "f" => {
                ui.filter.set(ui.filter.get().next());
                ui.selected.set(0);
                ui.selected_id.set(None);
                reset_scroll(ui);
            }
            Key::Character(text) if text == "1" => {
                ui.filter.set(FilterMode::Inbox);
                ui.selected.set(0);
                ui.selected_id.set(None);
                reset_scroll(ui);
            }
            Key::Character(text) if text == "2" => {
                ui.filter.set(FilterMode::All);
                ui.selected.set(0);
                ui.selected_id.set(None);
                reset_scroll(ui);
            }
            Key::Character(text) if text == "3" => {
                ui.filter.set(FilterMode::Pinned);
                ui.selected.set(0);
                ui.selected_id.set(None);
                reset_scroll(ui);
            }
            Key::Character(text) if text == "4" => {
                ui.filter.set(FilterMode::Archived);
                ui.selected.set(0);
                ui.selected_id.set(None);
                reset_scroll(ui);
            }
            Key::Character(text) if text == "i" => {
                ui.capture_open.set(true);
            }
            Key::Character(text) if text == "?" => {
                ui.help.set(true);
            }
            Key::Character(text) if text == "m" => {
                ui.process_selected.set(0);
                ui.process.set(true);
            }
            Key::Character(text) if text == "/" => {
                let search = ui.search;
                request_frame(move |_| search.focus());
            }
            _ => {}
        }
    })
}

fn current_capture_id(session: &Session, ui: UiState) -> Option<String> {
    ui.detail.get().or_else(|| selected_capture(session, ui))
}

fn selected_capture(session: &Session, ui: UiState) -> Option<String> {
    let model = session.model().get();
    let entries = filtered_entries(&model, &ui.query.get(), ui.filter.get());
    match entries.get(ui.selected.get()).map(|entry| entry.kind) {
        Some(EntryKind::Capture(capture)) => Some(capture.id.clone()),
        _ => None,
    }
}

fn set_surface(_layer: &Rc<RefCell<Option<LayerHandle>>>, _detail: bool) {
    // The list and detail share one panel size. Resizing the layer mid-view
    // flickers on niri, so the surface never changes size.
}
fn scroll_detail(ui: UiState, delta: f32) {
    let at = ui.detail_scroll.scroll_offset().y;
    ui.detail_scroll.scroll_to(0.0, (at + delta).max(0.0));
}

fn reset_scroll(ui: UiState) {
    ui.scroll_y.set(0.0);
    ui.list.scroll_to(0.0, 0.0);
}

fn move_selection(session: &Session, ui: UiState, delta: isize) {
    let model = session.model().get();
    let query = ui.query.get();
    let entries = filtered_entries(&model, &query, ui.filter.get());
    let count = entries.len();
    if count == 0 {
        ui.selected.set(0);
        ui.selected_id.set(None);
        return;
    }
    let current = ui.selected.get().min(count - 1);
    let next = (current as isize + delta).clamp(0, count as isize - 1) as usize;
    ui.selected.set(next);
    ui.selected_id.set(entries.get(next).map(entry_key));
}

fn surface(child: Div) -> Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(child)
}

// ---------------------------------------------------------------------------
// List
// ---------------------------------------------------------------------------

fn live_surface(session: &Session, ui: UiState, layer: Rc<RefCell<Option<LayerHandle>>>) -> Div {
    let list_session = session.clone();
    let detail_session = session.clone();
    let list_layer = layer.clone();
    let detail_layer = layer.clone();
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(show(
            move || ui.detail.get().is_none(),
            move || live_panel(&list_session, ui, list_layer.clone()),
        ))
        .child(show(
            move || ui.detail.get().is_some(),
            move || detail_panel(&detail_session, ui, detail_layer.clone()),
        ))
}

/// A scrim and one sheet, clipped to the rounded panel so the transparent
/// shadow margin stays transparent.
fn hud_overlays(
    session: &Session,
    ui: UiState,
    layer: Rc<RefCell<Option<LayerHandle>>>,
) -> impl IntoView {
    let session = session.clone();
    let show_any = {
        let ui = ui;
        move || {
            ui.command.get()
                || ui.confirm.get().is_some()
                || ui.reply.get().is_some()
                || ui.capture_open.get()
                || ui.help.get()
                || ui.process.get()
        }
    };
    show(show_any, move || {
        div()
            .absolute()
            .inset(0.0_f32)
            .z_index(20)
            .child(show(move || ui.process.get(), {
                let session = session.clone();
                let layer = layer.clone();
                move || {
                    div()
                        .absolute()
                        .inset(0.0_f32)
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(Color::from_rgba(0x00000080))
                        .child(process_sheet(&session, ui, layer.clone()))
                }
            }))
            .child(show(move || ui.command.get(), {
                let session = session.clone();
                let layer = layer.clone();
                move || {
                    div()
                        .absolute()
                        .inset(0.0_f32)
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(Color::from_rgba(0x00000080))
                        .child(command_menu(&session, ui, layer.clone()))
                }
            }))
            .child(show(move || ui.capture_open.get(), {
                let session = session.clone();
                move || {
                    div()
                        .absolute()
                        .inset(0.0_f32)
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(Color::from_rgba(0x00000080))
                        .child(compose_sheet("新しいCapture", None, None, &session, {
                            let ui = ui;
                            move || ui.capture_open.set(false)
                        }))
                }
            }))
            .child(show(move || ui.reply.get().is_some(), {
                let session = session.clone();
                move || {
                    let id = ui.reply.get();
                    let quote = id.as_ref().and_then(|id| {
                        session
                            .model()
                            .get()
                            .captures
                            .iter()
                            .find(|capture| capture.id == *id)
                            .map(|capture| preview(&capture.body, 46, 1).0)
                    });
                    let reply_to = id.clone();
                    div()
                        .absolute()
                        .inset(0.0_f32)
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(Color::from_rgba(0x00000080))
                        .child(compose_sheet("返信", quote, reply_to, &session, {
                            let ui = ui;
                            move || ui.reply.set(None)
                        }))
                }
            }))
            .child(show(move || ui.confirm.get().is_some(), {
                let session = session.clone();
                let layer = layer.clone();
                move || {
                    div()
                        .absolute()
                        .inset(0.0_f32)
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(Color::from_rgba(0x00000099))
                        .child(confirm_sheet(&session, ui, layer.clone()))
                }
            }))
            .child(show(move || ui.help.get(), {
                move || {
                    div()
                        .absolute()
                        .inset(0.0_f32)
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(Color::from_rgba(0x00000080))
                        .child(help_sheet(ui))
                }
            }))
    })
}

/// Everything between the status bar and the action bar. Data changes rebuild
/// only the list and the status line, so the search field keeps its text.
fn live_panel(session: &Session, ui: UiState, layer: Rc<RefCell<Option<LayerHandle>>>) -> Div {
    let model = session.model();
    let filter = ui.filter;
    let label = text(move || filter.get().label().to_owned())
        .font_family(SANS)
        .text_size(12.0_f32)
        .w(58.0_f32)
        .text_color(rgb(TEXT_SECONDARY));
    let count = text({
        let model = model.clone();
        move || filter_count(&model.get(), filter.get()).to_string()
    })
    .font_family(MONO)
    .text_size(10.5_f32)
    .w(24.0_f32)
    .text_color(rgb(TEXT_MUTED));

    panel_frame(
        header(label, count, ui),
        scroll_list(
            ui,
            dyn_view({
                let model = model.clone();
                move || list(&model.get(), ui)
            }),
        ),
        action_bar_live(session, ui, layer.clone()),
        dyn_view({
            let model = model.clone();
            move || status_bar(&model.get())
        }),
        hud_overlays(session, ui, layer),
    )
}

fn panel_frame(
    header: Div,
    list: impl IntoView,
    actions: Div,
    status: impl IntoView,
    overlay: impl IntoView,
) -> Div {
    div()
        .w(PANEL_W)
        .h(PANEL_H)
        .rounded(18.0_f32)
        .bg(rgb(PANEL))
        .border(1.0_f32)
        .border_color(rgb(BORDER))
        .shadow(BoxShadow {
            offset_y: 24.0,
            blur: 64.0,
            color: Color::from_rgba(0x05030888),
            ..Default::default()
        })
        .overflow_hidden()
        .flex()
        .flex_col()
        .relative()
        .child(header)
        .child(list)
        .child(actions)
        .child(status)
        .child(overlay)
}

fn static_panel(model: &Model, ui: UiState) -> Div {
    panel_frame(
        header(
            ui_text("Inbox", 12.0, TEXT_SECONDARY),
            mono_count(model.inbox_len()),
            ui,
        ),
        scroll_list(ui, list(model, ui)),
        action_bar(),
        status_bar(model),
        div(),
    )
}

fn mono_count(value: usize) -> Text {
    mono_text(value.to_string(), 10.5, TEXT_MUTED)
}

fn header(label: Text, count: Text, ui: UiState) -> Div {
    div()
        .h(64.0_f32)
        .w_full()
        .flex_none()
        .px(16.0_f32)
        .flex()
        .flex_row()
        .items_center()
        .gap(10.0_f32)
        .child(mark())
        .child(
            div()
                .flex_1()
                .h(40.0_f32)
                .min_w(0.0_f32)
                .rounded(9.0_f32)
                .bg(rgb(RAISED))
                .border(1.0_f32)
                .border_color(rgb(BORDER))
                .flex()
                .flex_row()
                .items_center()
                .gap(8.0_f32)
                .px(12.0_f32)
                .child(icon(&icons::SEARCH, 15.0_f32).text_color(rgb(TEXT_MUTED)))
                .child(
                    text_input()
                        .placeholder("検索")
                        .value(ui.query)
                        .on_input(move |text| {
                            ui.query.set(text.to_owned());
                            ui.selected.set(0);
                            ui.selected_id.set(None);
                        })
                        .on_focus_change(move |focused| ui.search_focused.set(focused))
                        .node_ref(ui.search)
                        .font_family(SANS)
                        .text_size(13.5_f32)
                        .flex_1()
                        .min_w(0.0_f32)
                        .text_color(rgb(TEXT)),
                )
                .child(
                    div()
                        .w(20.0_f32)
                        .h(20.0_f32)
                        .rounded(4.0_f32)
                        .bg(rgb(BORDER))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            text(move || {
                                if ui.search_focused.get() {
                                    "Tab".to_owned()
                                } else {
                                    "/".to_owned()
                                }
                            })
                            .font_family(MONO)
                            .text_size(9.0_f32)
                            .text_color(rgb(TEXT_SECONDARY)),
                        ),
                ),
        )
        .child(
            div()
                .h(40.0_f32)
                .w(132.0_f32)
                .px(10.0_f32)
                .rounded(9.0_f32)
                .bg(rgb(RAISED))
                .flex()
                .flex_row()
                .items_center()
                .gap(4.0_f32)
                .cursor(CursorIcon::Pointer)
                .on_click(move |_| {
                    ui.filter.set(ui.filter.get().next());
                    ui.selected.set(0);
                    ui.selected_id.set(None);
                    reset_scroll(ui);
                })
                .child(label)
                .child(count)
                .child(icon(&icons::CHEVRON_RIGHT, 12.0_f32).text_color(rgb(TEXT_MUTED))),
        )
        .child(
            div()
                .w(40.0_f32)
                .h(40.0_f32)
                .rounded(9.0_f32)
                .bg(rgb(0x241e31))
                .flex()
                .items_center()
                .justify_center()
                .cursor(CursorIcon::Pointer)
                .on_click(move |_| ui.capture_open.set(true))
                .child(icon(&icons::ADD, 16.0_f32).text_color(rgb(ACCENT))),
        )
}

fn keycap(label: &str) -> Div {
    div()
        .h(16.0_f32)
        .px(4.0_f32)
        .rounded(4.0_f32)
        .bg(rgb(BORDER))
        .flex()
        .items_center()
        .justify_center()
        .child(mono_text(label.to_owned(), 8.5, TEXT_SECONDARY))
}

fn mark() -> Div {
    div()
        .w(18.0_f32)
        .h(18.0_f32)
        .relative()
        .flex_none()
        .child(
            div()
                .absolute()
                .left(2.0_f32)
                .top(9.0_f32)
                .w(4.0_f32)
                .h(4.0_f32)
                .rounded(2.0_f32)
                .bg(rgb(ACCENT)),
        )
        .child(
            div()
                .absolute()
                .left(7.0_f32)
                .top(3.0_f32)
                .w(6.0_f32)
                .h(6.0_f32)
                .rounded(3.0_f32)
                .bg(Color::from_rgba(0xd9c0ffe6)),
        )
        .child(
            div()
                .absolute()
                .left(13.0_f32)
                .top(12.0_f32)
                .w(3.0_f32)
                .h(3.0_f32)
                .rounded(2.0_f32)
                .bg(Color::from_rgba(0xd9c0ffa6)),
        )
}

struct Row {
    time: String,
    body: String,
    height: f32,
    selected: bool,
    trailer: Option<String>,
    pin: bool,
    pending: bool,
    archived: bool,
    reply_parent: Option<String>,
}

struct Entry<'a> {
    micros: i64,
    body: &'a str,
    kind: EntryKind<'a>,
}

#[derive(Clone, Copy)]
enum EntryKind<'a> {
    Capture(&'a Capture),
    Pending,
}

fn entry_key(entry: &Entry<'_>) -> String {
    match entry.kind {
        EntryKind::Capture(capture) => capture.id.clone(),
        EntryKind::Pending => format!("pending:{}:{}", entry.micros, entry.body),
    }
}

fn entries(model: &Model) -> Vec<Entry<'_>> {
    let mut entries: Vec<Entry<'_>> = model
        .pending
        .iter()
        .map(|pending| Entry {
            micros: pending.created_at_micros,
            body: &pending.body,
            kind: EntryKind::Pending,
        })
        .collect();
    for capture in model.captures.iter() {
        entries.push(Entry {
            micros: capture.created_at(),
            body: &capture.body,
            kind: EntryKind::Capture(capture),
        });
    }
    entries.sort_by(|left, right| right.micros.cmp(&left.micros));
    entries
}

fn filtered_entries<'a>(model: &'a Model, query: &str, filter: FilterMode) -> Vec<Entry<'a>> {
    let query = query.trim().to_lowercase();
    let mut entries = entries(model);
    entries.retain(|entry| match entry.kind {
        EntryKind::Capture(capture) => filter.matches(capture),
        EntryKind::Pending => matches!(filter, FilterMode::Inbox | FilterMode::All),
    });
    if !query.is_empty() {
        entries.retain(|entry| match entry.kind {
            EntryKind::Capture(capture) => capture.matches(&query),
            EntryKind::Pending => entry.body.to_lowercase().contains(&query),
        });
    }
    entries
}

enum ListItem {
    Node(Div, f32),
    Entry(usize, f32),
}

fn list(model: &Model, ui: UiState) -> Div {
    let entries = filtered_entries(model, &ui.query.get(), ui.filter.get());
    let mut selected = ui.selected.get();
    if entries.is_empty() {
        selected = 0;
        ui.selected_id.set(None);
    } else {
        if let Some(id) = ui.selected_id.get()
            && let Some(position) = entries.iter().position(|entry| entry_key(entry) == id)
        {
            selected = position;
        }
        if selected >= entries.len() {
            selected = entries.len() - 1;
        }
        let id = entry_key(&entries[selected]);
        if ui.selected_id.get().as_deref() != Some(id.as_str()) {
            ui.selected_id.set(Some(id));
        }
    }
    ui.selected.set(selected);
    let scroll = ui.scroll_y.get();

    if entries.is_empty() {
        return empty_state(model, ui.filter.get(), &ui.query.get());
    }

    let mut fresh_counts: HashMap<(i32, u8, u8), usize> = HashMap::new();
    for entry in &entries {
        let when = local_at(entry.micros);
        let day = (when.year(), when.month() as u8, when.day());
        if entry.micros > ui.last_seen {
            *fresh_counts.entry(day).or_default() += 1;
        }
    }

    let mut items: Vec<ListItem> = Vec::new();
    let mut current_day: Option<(i32, u8, u8)> = None;
    let mut divider_done = ui.last_seen <= 0;
    let mut offset = 0.0_f32;
    let mut selected_offset = 0.0_f32;
    for (index, entry) in entries.iter().enumerate() {
        if !divider_done && entry.micros < ui.last_seen {
            items.push(ListItem::Node(
                last_seen_line(&format!("前回ここまで・{}", hhmm(ui.last_seen))),
                20.0,
            ));
            offset += 20.0;
            divider_done = true;
        }
        let time = local_at(entry.micros);
        let day = (time.year(), time.month() as u8, time.day());
        if current_day != Some(day) {
            let fresh = fresh_counts.get(&day).copied().unwrap_or(0);
            let fresh = (fresh > 0 && ui.last_seen > 0).then(|| format!("新着 {fresh}"));
            items.push(ListItem::Node(
                date_header(&date_label(time), fresh.as_deref()),
                32.0,
            ));
            offset += 32.0;
            current_day = Some(day);
        }
        let item = row_of(entry, index == selected, reply_parent_preview(model, entry));
        if index == selected {
            selected_offset = offset;
        }
        let height = item.height;
        items.push(ListItem::Entry(index, height));
        offset += height;
    }
    let total_height = offset;

    let view_start = (scroll - 200.0).max(0.0);
    let view_end = scroll + 520.0 + 200.0;
    let mut children: Vec<Div> = Vec::new();
    let mut at = 0.0_f32;
    let mut last_end = 0.0_f32;
    let mut top_added = false;
    for item in items {
        let (child, height) = match item {
            ListItem::Node(child, height) => (child, height),
            ListItem::Entry(index, height) => {
                let item = row_of(
                    &entries[index],
                    index == selected,
                    reply_parent_preview(model, &entries[index]),
                );
                let id = match entries[index].kind {
                    EntryKind::Capture(capture) => Some(capture.id.clone()),
                    EntryKind::Pending => None,
                };
                let was_selected = index == selected;
                let key = entry_key(&entries[index]);
                let row = row(&item).cursor(CursorIcon::Pointer).on_click(move |_| {
                    ui.selected.set(index);
                    ui.selected_id.set(Some(key.clone()));
                    if was_selected && let Some(id) = id.clone() {
                        ui.detail.set(Some(id));
                    }
                });
                (row, height)
            }
        };
        let start = at;
        at += height;
        if start + height < view_start {
            continue;
        }
        if start > view_end {
            break;
        }
        if !top_added {
            if start > 0.0 {
                children.push(div().h(start).flex_none());
            }
            top_added = true;
        }
        last_end = at;
        children.push(child);
    }
    if last_end < total_height {
        children.push(div().h(total_height - last_end).flex_none());
    }

    let list_ref = ui.list;
    let selected_height = entries
        .get(selected)
        .map(|entry| row_of(entry, true, reply_parent_preview(model, entry)).height)
        .unwrap_or(44.0);
    request_frame(move |_| {
        let at = list_ref.scroll_offset().y;
        let shown = 520.0;
        if selected_offset < at {
            list_ref.scroll_to(0.0, selected_offset);
        } else if selected_offset + selected_height > at + shown {
            list_ref.scroll_to(0.0, (selected_offset + selected_height - shown).max(0.0));
        }
    });
    div().w_full().flex().flex_col().children(children)
}

fn scroll_list(ui: UiState, content: impl IntoView) -> Div {
    div()
        .h(520.0_f32)
        .w_full()
        .flex_none()
        .px(16.0_f32)
        .node_ref(ui.list)
        .overflow_y_scroll()
        .on_scrolled(move |point| ui.scroll_y.set(point.y))
        .flex()
        .flex_col()
        .child(content)
}

fn empty_state(model: &Model, filter: FilterMode, query: &str) -> Div {
    let filter_message = match filter {
        FilterMode::Inbox => "Inbox は空",
        FilterMode::All => "Capture はありません",
        FilterMode::Pinned => "ピンはありません",
        FilterMode::Archived => "アーカイブは空",
    };
    let query = query.trim();
    let message = if !query.is_empty() {
        let mut short: String = query.chars().take(16).collect();
        if query.chars().count() > 16 {
            short.push('…');
        }
        format!("「{short}」に一致なし")
    } else {
        match model.status {
            Status::Connecting if model.captures.is_empty() && model.pending.is_empty() => {
                "読み込み中"
            }
            Status::Connecting => filter_message,
            Status::NoCredentials if model.captures.is_empty() => "mote connect で接続",
            Status::NoCredentials => filter_message,
            Status::Offline if model.captures.is_empty() && model.pending.is_empty() => {
                if model.from_cache {
                    "前回の一覧（接続なし）"
                } else {
                    "接続なし"
                }
            }
            Status::Offline | Status::Connected => filter_message,
        }
        .to_owned()
    };
    let mut state = div()
        .h(520.0_f32)
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(6.0_f32)
        .child(ui_text(message, 11.5, TEXT_MUTED));
    if !query.is_empty() {
        state = state.child(mono_text("Esc で検索解除", 9.5, TEXT_MUTED));
    }
    state
}

fn reply_parent_preview(model: &Model, entry: &Entry<'_>) -> Option<String> {
    let capture = match entry.kind {
        EntryKind::Capture(capture) => capture,
        EntryKind::Pending => return None,
    };
    let parent_id = capture.reply_to.as_ref()?;
    Some(
        match model
            .captures
            .iter()
            .find(|candidate| candidate.id == *parent_id)
        {
            Some(parent) => preview(&parent.body, 36, 1).0,
            None => "返信先は見つかりません".to_owned(),
        },
    )
}

fn row_of(entry: &Entry<'_>, selected: bool, reply_parent: Option<String>) -> Row {
    let (body, lines, truncated) = preview(entry.body, 34, 3);
    let character_count = entry.body.chars().count();
    let capture = match entry.kind {
        EntryKind::Capture(capture) => Some(capture),
        EntryKind::Pending => None,
    };
    let parent_height = if reply_parent.is_some() { 16.0 } else { 0.0 };
    Row {
        time: hhmm(entry.micros),
        height: (21.0_f32 + 19.0_f32 * lines as f32 + parent_height).max(44.0_f32),
        selected,
        trailer: truncated.then(|| format!("{}字", grouped(character_count))),
        pin: capture.is_some_and(|capture| capture.pinned),
        pending: matches!(entry.kind, EntryKind::Pending),
        archived: capture.is_some_and(|capture| capture.archived),
        reply_parent,
        body,
    }
}

fn preview(body: &str, columns: usize, max_lines: usize) -> (String, usize, bool) {
    let flattened = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let characters: Vec<char> = flattened.chars().collect();
    let limit = columns * max_lines;
    let truncated = characters.len() > limit;
    let mut preview: String = characters.into_iter().take(limit).collect();
    if truncated {
        preview.push('…');
    }
    let lines = preview
        .chars()
        .count()
        .div_ceil(columns)
        .clamp(1, max_lines);
    (preview, lines, truncated)
}

fn grouped(value: usize) -> String {
    let digits = value.to_string();
    let mut grouped = String::new();
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(character);
    }
    grouped
}

fn last_seen_line(label: &str) -> Div {
    let line = || div().h(1.0_f32).flex_1().bg(rgb(BORDER));
    div()
        .h(20.0_f32)
        .w_full()
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(10.0_f32)
        .child(line())
        .child(mono_text(label.to_owned(), 9.5, TEXT_MUTED))
        .child(line())
}

fn date_header(date: &str, fresh: Option<&str>) -> Div {
    let mut header = div()
        .h(32.0_f32)
        .w_full()
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(8.0_f32)
        .child(ui_text(date.to_owned(), 12.5, 0xece7f3).font_weight(FontWeight::SEMIBOLD));
    header = header.child(div().flex_1());
    if let Some(fresh) = fresh {
        header = header.child(mono_text(fresh.to_owned(), 10.5, TEXT_MUTED));
    }
    header
}

fn row(item: &Row) -> Div {
    let mut content = div()
        .flex_1()
        .min_w(0.0_f32)
        .self_start()
        .pt(11.0_f32)
        .opacity(if item.archived { 0.62_f32 } else { 1.0_f32 })
        .flex()
        .flex_col()
        .overflow_hidden();
    if let Some(parent) = &item.reply_parent {
        content = content.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(5.0_f32)
                .child(icon(&icons::REPLY, 11.0_f32).text_color(rgb(TEXT_MUTED)))
                .child(
                    ui_text(parent.clone(), 10.0, TEXT_MUTED).line_height(LineHeight::Px(16.0_f32)),
                ),
        );
    }
    content = content.child(
        ui_text(item.body.clone(), 13.5, TEXT)
            .line_height(LineHeight::Px(19.0_f32))
            .w_full(),
    );

    let mut trailer = div()
        .flex_none()
        .self_start()
        .pt(11.0_f32)
        .flex()
        .flex_row()
        .items_center()
        .gap(6.0_f32);
    if let Some(words) = &item.trailer {
        trailer = trailer.child(mono_text(words.clone(), 10.5, TEXT_MUTED));
    }
    if item.pin {
        trailer = trailer.child(icon(&icons::BOOKMARK, 14.0_f32).text_color(rgb(TEXT_SECONDARY)));
    }
    if item.archived {
        trailer = trailer.child(icon(&icons::ARCHIVE, 13.0_f32).text_color(rgb(TEXT_MUTED)));
    }
    if item.pending {
        trailer = trailer.child(mono_text("未送信", 9.5, WARNING));
    }

    div()
        .h(item.height)
        .w_full()
        .flex_none()
        .overflow_hidden()
        .rounded(8.0_f32)
        .bg(if item.selected {
            rgb(SELECTED)
        } else {
            Color::TRANSPARENT
        })
        .flex()
        .flex_row()
        .items_center()
        .gap(10.0_f32)
        .child(
            div()
                .w(2.0_f32)
                .h(item.height - 20.0_f32)
                .flex_none()
                .rounded(1.0_f32)
                .bg(if item.selected {
                    rgb(ACCENT)
                } else {
                    Color::TRANSPARENT
                }),
        )
        .child(
            div()
                .w(44.0_f32)
                .flex_none()
                .self_start()
                .pt(11.0_f32)
                .flex()
                .items_center()
                .child(mono_text(item.time.clone(), 10.5, TEXT_SECONDARY)),
        )
        .child(content)
        .child(trailer)
}

fn action_bar_live(session: &Session, ui: UiState, layer: Rc<RefCell<Option<LayerHandle>>>) -> Div {
    let open_session = session.clone();
    let open_layer = layer.clone();
    let archive_session = session.clone();
    let archive_label_session = archive_session.clone();
    let menu_ui = ui;
    div()
        .h(56.0_f32)
        .w_full()
        .flex_none()
        .bg(rgb(ACTION_BAR))
        .px(16.0_f32)
        .flex()
        .flex_row()
        .items_center()
        .gap(6.0_f32)
        .child(
            div()
                .h(30.0_f32)
                .px(20.0_f32)
                .rounded(7.0_f32)
                .bg(rgb(0x241e31))
                .flex()
                .items_center()
                .cursor(CursorIcon::Pointer)
                .on_click(move |_| {
                    menu_ui.process_selected.set(0);
                    menu_ui.process.set(true);
                })
                .gap(6.0_f32)
                .child(ui_text("処理", 11.5, TEXT_SECONDARY))
                .child(keycap("m")),
        )
        .child(
            div()
                .h(30.0_f32)
                .px(11.0_f32)
                .rounded(7.0_f32)
                .flex()
                .items_center()
                .cursor(CursorIcon::Pointer)
                .on_click(move |_| {
                    if let Some(id) = current_capture_id(&archive_session, ui) {
                        session_archive_or_unarchive(&archive_session, &id);
                    }
                })
                .gap(6.0_f32)
                .child(
                    text({
                        let session = archive_label_session;
                        move || {
                            let archived = current_capture_id(&session, ui)
                                .and_then(|id| {
                                    session
                                        .model()
                                        .get()
                                        .captures
                                        .iter()
                                        .find(|capture| capture.id == id)
                                        .map(|capture| capture.archived)
                                })
                                .unwrap_or(false);
                            if archived {
                                "戻す"
                            } else {
                                "アーカイブ"
                            }
                            .to_owned()
                        }
                    })
                    .font_family(SANS)
                    .text_size(11.5_f32)
                    .text_color(rgb(TEXT_SECONDARY)),
                )
                .child(keycap("a")),
        )
        .child(
            div()
                .h(30.0_f32)
                .w(28.0_f32)
                .flex()
                .items_center()
                .justify_center()
                .cursor(CursorIcon::Pointer)
                .on_click(move |_| {
                    menu_ui.command.set(true);
                    menu_ui.command_query.set(String::new());
                    menu_ui.command_selected.set(0);
                    let search = menu_ui.command_search;
                    request_frame(move |_| search.focus());
                })
                .child(icon(&icons::MORE_HORIZ, 16.0_f32).text_color(rgb(TEXT_MUTED))),
        )
        .child(div().flex_1())
        .child(
            div()
                .h(32.0_f32)
                .px(24.0_f32)
                .rounded(8.0_f32)
                .bg(rgb(ACCENT))
                .flex()
                .items_center()
                .cursor(CursorIcon::Pointer)
                .on_click(move |_| {
                    if let Some(id) = current_capture_id(&open_session, ui) {
                        ui.detail.set(Some(id));
                        set_surface(&open_layer, true);
                    }
                })
                .gap(6.0_f32)
                .child(ui_text("開く", 11.5, ON_ACCENT).font_weight(FontWeight::SEMIBOLD))
                .child(
                    div()
                        .h(16.0_f32)
                        .px(4.0_f32)
                        .rounded(4.0_f32)
                        .bg(Color::from_rgba(0x17141f44))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(mono_text("l", 8.5, ON_ACCENT)),
                ),
        )
}

fn session_archive_or_unarchive(session: &Session, id: &str) {
    let archived = session
        .model()
        .get()
        .captures
        .iter()
        .find(|capture| capture.id == id)
        .is_some_and(|capture| capture.archived);
    if archived {
        session.unarchive(id);
    } else {
        session.archive(id);
    }
}

fn action_bar() -> Div {
    div()
        .h(56.0_f32)
        .w_full()
        .flex_none()
        .bg(rgb(ACTION_BAR))
        .px(16.0_f32)
        .flex()
        .flex_row()
        .items_center()
        .gap(6.0_f32)
        .child(
            div()
                .h(30.0_f32)
                .px(20.0_f32)
                .rounded(7.0_f32)
                .bg(rgb(0x241e31))
                .flex()
                .items_center()
                .child(ui_text("処理", 11.5, TEXT_SECONDARY)),
        )
        .child(
            div()
                .h(30.0_f32)
                .px(11.0_f32)
                .rounded(7.0_f32)
                .flex()
                .items_center()
                .child(ui_text("アーカイブ", 11.5, TEXT_SECONDARY)),
        )
        .child(
            div()
                .h(30.0_f32)
                .w(28.0_f32)
                .flex()
                .items_center()
                .justify_center()
                .child(icon(&icons::MORE_HORIZ, 16.0_f32).text_color(rgb(TEXT_MUTED))),
        )
        .child(div().flex_1())
        .child(
            div()
                .h(32.0_f32)
                .px(24.0_f32)
                .rounded(8.0_f32)
                .bg(rgb(ACCENT))
                .flex()
                .items_center()
                .gap(6.0_f32)
                .child(ui_text("開く", 11.5, ON_ACCENT).font_weight(FontWeight::SEMIBOLD))
                .child(
                    div()
                        .h(16.0_f32)
                        .px(4.0_f32)
                        .rounded(4.0_f32)
                        .bg(Color::from_rgba(0x17141f44))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(mono_text("l", 8.5, ON_ACCENT)),
                ),
        )
}

fn error_label(error: &str) -> &'static str {
    if error.contains("OAuth") || error.contains("authentication") || error.contains("認証") {
        "mote connect で再接続"
    } else if error.contains("Connection refused") || error.contains("Io(") {
        "ノードに接続できません"
    } else if error.contains("保存") || error.contains("削除") || error.contains("変更") {
        "操作を保存できませんでした"
    } else {
        "操作できませんでした"
    }
}

fn status_bar(model: &Model) -> Div {
    let healthy =
        model.status == Status::Connected && model.pending.is_empty() && model.error.is_none();
    let (color, label) = match model.status {
        Status::Connected => (SUCCESS, "接続済み"),
        Status::Connecting => (TEXT_MUTED, "接続中"),
        Status::Offline if model.from_cache => (WARNING, "前回の一覧"),
        Status::Offline => (WARNING, "オフライン"),
        Status::NoCredentials => (WARNING, "未接続"),
    };
    let color = if !model.pending.is_empty() || model.error.is_some() {
        WARNING
    } else {
        color
    };
    let mut bar = div()
        .h(40.0_f32)
        .w_full()
        .flex_none()
        .bg(rgb(STATUS_BAR))
        .px(16.0_f32)
        .flex()
        .flex_row()
        .items_center()
        .gap(7.0_f32)
        .child(div().w(6.0_f32).h(6.0_f32).rounded(3.0_f32).bg(rgb(color)));
    if !healthy {
        if let Some(error) = model.error.as_deref() {
            bar = bar.child(ui_text(error_label(error), 10.5, TEXT_MUTED));
        } else {
            bar = bar.child(ui_text(label, 10.5, TEXT_MUTED));
        }
    }
    if !model.pending.is_empty() {
        bar = bar.child(ui_text(
            format!("未送信 {}", model.pending.len()),
            10.5,
            WARNING,
        ));
    }
    if model.status == Status::NoCredentials {
        bar = bar.child(ui_text("mote connect", 10.5, WARNING));
    }
    bar.child(div().flex_1()).child(mono_text(
        "j/k 移動   J/K 5件   H/L フィルタ   r 返信   i Capture   Ctrl+K   ?",
        10.5,
        TEXT_MUTED,
    ))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuAction {
    Open,
    Reply,
    Archive,
    Unarchive,
    Pin,
    Copy,
    Delete,
    Close,
}

impl MenuAction {
    fn label(self, pinned: bool) -> &'static str {
        match self {
            MenuAction::Open => "開く",
            MenuAction::Reply => "返信",
            MenuAction::Archive => "アーカイブ",
            MenuAction::Unarchive => "戻す",
            MenuAction::Pin => {
                if pinned {
                    "ピン解除"
                } else {
                    "ピン"
                }
            }
            MenuAction::Copy => "コピー",
            MenuAction::Delete => "削除",
            MenuAction::Close => "閉じる",
        }
    }
}

fn current_capture(session: &Session, ui: UiState) -> Option<Capture> {
    let id = current_capture_id(session, ui)?;
    session
        .model()
        .get()
        .captures
        .iter()
        .find(|capture| capture.id == id)
        .cloned()
}

fn command_actions(session: &Session, ui: UiState) -> Vec<MenuAction> {
    let mut actions = Vec::new();
    if current_capture_id(session, ui).is_some() {
        let archived = current_capture(session, ui).is_some_and(|capture| capture.archived);
        actions.extend([
            MenuAction::Open,
            MenuAction::Reply,
            if archived {
                MenuAction::Unarchive
            } else {
                MenuAction::Archive
            },
            MenuAction::Pin,
            MenuAction::Copy,
            MenuAction::Delete,
        ]);
    }
    actions.push(MenuAction::Close);
    actions
}

fn filtered_actions(session: &Session, ui: UiState) -> Vec<MenuAction> {
    let query = ui.command_query.get().to_lowercase();
    let pinned = current_capture(session, ui).is_some_and(|capture| capture.pinned);
    command_actions(session, ui)
        .into_iter()
        .filter(|action| query.is_empty() || action.label(pinned).to_lowercase().contains(&query))
        .collect()
}

fn command_at(session: &Session, ui: UiState) -> Option<MenuAction> {
    let actions = filtered_actions(session, ui);
    actions
        .get(
            ui.command_selected
                .get()
                .min(actions.len().saturating_sub(1)),
        )
        .copied()
}

fn move_command(session: Session, ui: UiState, delta: isize) {
    let count = filtered_actions(&session, ui).len();
    if count == 0 {
        ui.command_selected.set(0);
        return;
    }
    let current = ui.command_selected.get().min(count - 1);
    let next = (current as isize + delta).clamp(0, count as isize - 1) as usize;
    ui.command_selected.set(next);
}

fn run_command(
    action: MenuAction,
    session: &Session,
    ui: UiState,
    layer: &Rc<RefCell<Option<LayerHandle>>>,
) {
    match action {
        MenuAction::Open => {
            if let Some(id) = current_capture_id(session, ui) {
                ui.detail.set(Some(id));
                set_surface(layer, true);
            }
        }
        MenuAction::Reply => {
            if let Some(id) = current_capture_id(session, ui) {
                ui.reply.set(Some(id));
            }
        }
        MenuAction::Archive => {
            if let Some(id) = current_capture_id(session, ui) {
                session.archive(&id);
                if ui.detail.get().is_some() {
                    ui.detail.set(None);
                }
            }
        }
        MenuAction::Unarchive => {
            if let Some(id) = current_capture_id(session, ui) {
                session.unarchive(&id);
            }
        }
        MenuAction::Pin => {
            if let Some(capture) = current_capture(session, ui) {
                session.pin(&capture.id, !capture.pinned);
            }
        }
        MenuAction::Copy => {
            if let Some(capture) = current_capture(session, ui) {
                copy_text(copy_body(&capture));
            }
        }
        MenuAction::Delete => {
            if let Some(id) = current_capture_id(session, ui) {
                ui.confirm_delete.set(false);
                ui.confirm.set(Some(id));
            }
        }
        MenuAction::Close => {
            ui.command.set(false);
            ui.root.focus();
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProcessAction {
    Complete,
    Copy,
    Reply,
    Close,
}

impl ProcessAction {
    const ALL: [Self; 4] = [Self::Complete, Self::Copy, Self::Reply, Self::Close];

    fn label(self) -> &'static str {
        match self {
            Self::Complete => "完了",
            Self::Copy => "コピー",
            Self::Reply => "返信",
            Self::Close => "閉じる",
        }
    }

    fn shortcut(self) -> &'static str {
        match self {
            Self::Complete => "a",
            Self::Copy => "c",
            Self::Reply => "r",
            Self::Close => "Esc",
        }
    }
}

fn run_process(
    action: ProcessAction,
    session: &Session,
    ui: UiState,
    layer: &Rc<RefCell<Option<LayerHandle>>>,
) {
    match action {
        ProcessAction::Complete => {
            if let Some(id) = current_capture_id(session, ui) {
                session.archive(&id);
                if ui.detail.get().is_some() {
                    ui.detail.set(None);
                    set_surface(layer, false);
                }
            }
        }
        ProcessAction::Copy => {
            if let Some(capture) = current_capture(session, ui) {
                copy_text(copy_body(&capture));
            }
        }
        ProcessAction::Reply => {
            if let Some(id) = current_capture_id(session, ui) {
                ui.reply.set(Some(id));
            }
        }
        ProcessAction::Close => {
            ui.process.set(false);
            ui.root.focus();
        }
    }
}

fn process_sheet(session: &Session, ui: UiState, layer: Rc<RefCell<Option<LayerHandle>>>) -> Div {
    let selected = ui.process_selected.get().min(ProcessAction::ALL.len() - 1);
    let mut rows = div().w_full().flex().flex_col();
    for (index, action) in ProcessAction::ALL.iter().enumerate() {
        let action = *action;
        let session = session.clone();
        let layer = layer.clone();
        rows = rows.child(
            div()
                .h(44.0_f32)
                .w_full()
                .flex_none()
                .px(20.0_f32)
                .flex()
                .items_center()
                .bg(if index == selected {
                    rgb(SELECTED)
                } else {
                    Color::TRANSPARENT
                })
                .cursor(CursorIcon::Pointer)
                .on_click(move |_| {
                    ui.process.set(false);
                    run_process(action, &session, ui, &layer);
                    ui.root.focus();
                })
                .child(ui_text(action.label(), 12.0, TEXT))
                .child(div().flex_1())
                .child(mono_text(action.shortcut(), 10.5, TEXT_MUTED)),
        );
    }
    div()
        .w(600.0_f32)
        .h(248.0_f32)
        .rounded(16.0_f32)
        .bg(rgb(PANEL))
        .border(1.0_f32)
        .border_color(rgb(BORDER))
        .shadow(BoxShadow {
            offset_y: 20.0,
            blur: 56.0,
            color: Color::from_rgba(0x05030888),
            ..Default::default()
        })
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(
            div()
                .h(56.0_f32)
                .flex_none()
                .px(20.0_f32)
                .flex()
                .items_center()
                .child(ui_text("処理", 13.0, TEXT).font_weight(FontWeight::SEMIBOLD))
                .child(div().flex_1())
                .child(
                    div()
                        .cursor(CursorIcon::Pointer)
                        .on_click(move |_| {
                            ui.process.set(false);
                            ui.root.focus();
                        })
                        .child(icon(&icons::CLOSE, 16.0_f32).text_color(rgb(TEXT_MUTED))),
                ),
        )
        .child(rows)
}

fn confirm_sheet(session: &Session, ui: UiState, layer: Rc<RefCell<Option<LayerHandle>>>) -> Div {
    let target = ui.confirm.get().and_then(|id| {
        session
            .model()
            .get()
            .captures
            .iter()
            .find(|capture| capture.id == id)
            .cloned()
    });
    let delete_selected = ui.confirm_delete.get();
    let mut body = div()
        .flex_1()
        .px(24.0_f32)
        .py(20.0_f32)
        .flex()
        .flex_col()
        .gap(8.0_f32)
        .child(ui_text("削除", 13.0, TEXT).font_weight(FontWeight::SEMIBOLD))
        .child(ui_text("この Capture を削除しますか", 11.5, TEXT_MUTED));
    if let Some(capture) = &target {
        body = body.child(
            div()
                .w_full()
                .rounded(7.0_f32)
                .bg(rgb(0x1d1927))
                .px(10.0_f32)
                .py(7.0_f32)
                .flex()
                .flex_col()
                .gap(3.0_f32)
                .child(mono_text(hhmm(capture.created_at()), 9.5, TEXT_MUTED))
                .child(ui_text(
                    preview(&capture.body, 44, 1).0,
                    11.0,
                    TEXT_SECONDARY,
                )),
        );
    }
    body = body.child(mono_text("Tab 切り替え · Enter 決定", 9.5, TEXT_MUTED));

    div()
        .w(420.0_f32)
        .h(270.0_f32)
        .rounded(14.0_f32)
        .bg(rgb(PANEL))
        .border(1.0_f32)
        .border_color(rgb(BORDER))
        .shadow(BoxShadow {
            offset_y: 18.0,
            blur: 48.0,
            color: Color::from_rgba(0x05030888),
            ..Default::default()
        })
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(body)
        .child(
            div()
                .h(56.0_f32)
                .w_full()
                .flex_none()
                .px(20.0_f32)
                .flex()
                .flex_row()
                .items_center()
                .gap(8.0_f32)
                .child(div().flex_1())
                .child(
                    div()
                        .h(30.0_f32)
                        .px(14.0_f32)
                        .rounded(7.0_f32)
                        .bg(if delete_selected {
                            Color::TRANSPARENT
                        } else {
                            rgb(SELECTED)
                        })
                        .flex()
                        .items_center()
                        .cursor(CursorIcon::Pointer)
                        .on_click(move |_| {
                            ui.confirm.set(None);
                            ui.confirm_delete.set(false);
                            ui.root.focus();
                        })
                        .child(ui_text("キャンセル", 11.5, TEXT_SECONDARY)),
                )
                .child({
                    let session = session.clone();
                    let layer = layer.clone();
                    div()
                        .h(30.0_f32)
                        .px(16.0_f32)
                        .rounded(7.0_f32)
                        .bg(if delete_selected {
                            rgb(WARNING)
                        } else {
                            rgb(0x241e31)
                        })
                        .flex()
                        .items_center()
                        .cursor(CursorIcon::Pointer)
                        .on_click(move |_| {
                            if let Some(id) = ui.confirm.get() {
                                session.delete(&id);
                                ui.confirm.set(None);
                                ui.confirm_delete.set(false);
                                ui.detail.set(None);
                                set_surface(&layer, false);
                                ui.root.focus();
                            }
                        })
                        .child(ui_text(
                            "削除",
                            11.5,
                            if delete_selected {
                                ON_ACCENT
                            } else {
                                TEXT_SECONDARY
                            },
                        ))
                }),
        )
}

fn help_sheet(ui: UiState) -> Div {
    let keys = [
        ("j / k", "移動"),
        ("J / K", "5件移動"),
        ("l / Enter", "詳細"),
        ("h", "詳細から戻る"),
        ("/", "検索"),
        ("Tab", "検索から一覧へ"),
        ("i", "Capture"),
        ("r", "返信"),
        ("R", "未送信を再送"),
        ("a", "アーカイブ / 戻す"),
        ("u", "アーカイブから戻す"),
        ("H / L", "フィルタ移動"),
        ("p", "ピン"),
        ("c", "コピー"),
        ("d", "削除"),
        ("m", "処理"),
        ("Ctrl+K", "アクション"),
        ("1–4 / f", "Inbox / All / Pinned / Archived"),
        ("Esc", "戻る / 閉じる"),
        ("?", "ヘルプ"),
    ];
    let mut columns = div()
        .flex_1()
        .min_h(0.0_f32)
        .w_full()
        .flex()
        .flex_row()
        .gap(16.0_f32)
        .px(20.0_f32)
        .py(8.0_f32);
    for chunk in keys.chunks(10) {
        let mut column = div().flex_1().flex().flex_col();
        for &(key, label) in chunk {
            column = column.child(
                div()
                    .h(32.0_f32)
                    .w_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .child(mono_text(key, 11.0, TEXT_SECONDARY))
                    .child(div().flex_1())
                    .child(ui_text(label, 11.0, TEXT_MUTED)),
            );
        }
        columns = columns.child(column);
    }
    let body = columns;

    div()
        .w(560.0_f32)
        .h(520.0_f32)
        .rounded(18.0_f32)
        .bg(rgb(PANEL))
        .border(1.0_f32)
        .border_color(rgb(BORDER))
        .shadow(BoxShadow {
            offset_y: 24.0,
            blur: 64.0,
            color: Color::from_rgba(0x05030888),
            ..Default::default()
        })
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(
            div()
                .h(56.0_f32)
                .w_full()
                .flex_none()
                .px(20.0_f32)
                .flex()
                .items_center()
                .child(ui_text("キー", 13.0, TEXT).font_weight(FontWeight::SEMIBOLD))
                .child(div().flex_1())
                .child(
                    div()
                        .cursor(CursorIcon::Pointer)
                        .on_click(move |_| {
                            ui.help.set(false);
                            ui.root.focus();
                        })
                        .child(icon(&icons::CLOSE, 16.0_f32).text_color(rgb(TEXT_MUTED))),
                ),
        )
        .child(body)
}

fn command_menu(session: &Session, ui: UiState, layer: Rc<RefCell<Option<LayerHandle>>>) -> Div {
    let list_session = session.clone();
    div()
        .w(560.0_f32)
        .h(380.0_f32)
        .rounded(18.0_f32)
        .bg(rgb(PANEL))
        .border(1.0_f32)
        .border_color(rgb(BORDER))
        .shadow(BoxShadow {
            offset_y: 24.0,
            blur: 64.0,
            color: Color::from_rgba(0x05030888),
            ..Default::default()
        })
        .overflow_hidden()
        .flex()
        .flex_col()
        .relative()
        .child(
            div()
                .h(56.0_f32)
                .w_full()
                .flex_none()
                .px(16.0_f32)
                .flex()
                .items_center()
                .child(
                    div()
                        .h(36.0_f32)
                        .w_full()
                        .rounded(9.0_f32)
                        .bg(rgb(RAISED))
                        .border(1.0_f32)
                        .border_color(rgb(BORDER))
                        .flex()
                        .items_center()
                        .gap(8.0_f32)
                        .px(12.0_f32)
                        .child(icon(&icons::SEARCH, 14.0_f32).text_color(rgb(TEXT_MUTED)))
                        .child(
                            text_input()
                                .placeholder("アクションを検索")
                                .value(ui.command_query)
                                .node_ref(ui.command_search)
                                .on_input(move |text| {
                                    ui.command_query.set(text.to_owned());
                                    ui.command_selected.set(0);
                                })
                                .font_family(SANS)
                                .text_size(13.0_f32)
                                .flex_1()
                                .min_w(0.0_f32)
                                .text_color(rgb(TEXT)),
                        ),
                ),
        )
        .child(dyn_view(move || {
            command_list(&list_session, ui, layer.clone())
        }))
}

fn command_list(session: &Session, ui: UiState, layer: Rc<RefCell<Option<LayerHandle>>>) -> Div {
    let actions = filtered_actions(session, ui);
    let pinned = current_capture(session, ui).is_some_and(|capture| capture.pinned);
    if actions.is_empty() {
        return div()
            .flex_1()
            .w_full()
            .flex()
            .justify_center()
            .py(20.0_f32)
            .child(ui_text("該当なし", 11.0, TEXT_MUTED));
    }
    let selected = ui.command_selected.get().min(actions.len() - 1);
    let mut rows = div()
        .flex_1()
        .min_h(0.0_f32)
        .w_full()
        .overflow_y_scroll()
        .scrollbar_color(Color::from_rgba(0x8f84a166))
        .flex()
        .flex_col()
        .child(
            div()
                .h(28.0_f32)
                .w_full()
                .flex_none()
                .px(16.0_f32)
                .flex()
                .items_center()
                .child(ui_text("この項目", 10.5, TEXT_MUTED).font_weight(FontWeight::SEMIBOLD)),
        );
    for (index, action) in actions.iter().enumerate() {
        let session = session.clone();
        let layer = layer.clone();
        let action = *action;
        rows = rows.child(
            div()
                .h(40.0_f32)
                .w_full()
                .flex_none()
                .px(16.0_f32)
                .flex()
                .items_center()
                .bg(if index == selected {
                    rgb(SELECTED)
                } else {
                    Color::TRANSPARENT
                })
                .cursor(CursorIcon::Pointer)
                .on_click(move |_| {
                    ui.command.set(false);
                    run_command(action, &session, ui, &layer);
                    ui.root.focus();
                })
                .child(ui_text(action.label(pinned), 12.0, TEXT)),
        );
    }
    rows
}

fn detail_panel(session: &Session, ui: UiState, layer: Rc<RefCell<Option<LayerHandle>>>) -> Div {
    let Some(id) = ui.detail.get() else {
        return div();
    };
    let model = session.model().get();
    let Some(capture) = model.captures.iter().find(|capture| capture.id == id) else {
        return div()
            .w(DETAIL_W)
            .h(DETAIL_H)
            .rounded(18.0_f32)
            .bg(rgb(PANEL))
            .border(1.0_f32)
            .border_color(rgb(BORDER))
            .flex()
            .items_center()
            .justify_center()
            .child(ui_text("見つかりません", 11.5, TEXT_MUTED));
    };
    let at = local_at(capture.created_at());
    let mut body = div()
        .flex_1()
        .min_h(0.0_f32)
        .node_ref(ui.detail_scroll)
        .overflow_y_scroll()
        .scrollbar_color(Color::from_rgba(0x8f84a166))
        .px(24.0_f32)
        .py(24.0_f32)
        .flex()
        .flex_col()
        .gap(16.0_f32)
        .child(ui_text(date_label(at), 10.5, TEXT_MUTED))
        .child(mono_text(hhmm(capture.created_at()), 20.0, TEXT).font_weight(FontWeight::SEMIBOLD))
        .child(ui_text(
            format!(
                "{}字{}",
                grouped(capture.body.chars().count()),
                if capture.long { "・長文" } else { "" }
            ),
            10.5,
            TEXT_MUTED,
        ));
    if let Some(parent_id) = capture.reply_to.as_ref() {
        let parent = model
            .captures
            .iter()
            .find(|candidate| candidate.id == *parent_id);
        body = body.child(
            div()
                .w_full()
                .rounded(7.0_f32)
                .bg(rgb(0x1d1927))
                .px(12.0_f32)
                .py(8.0_f32)
                .flex()
                .flex_col()
                .gap(4.0_f32)
                .child(mono_text(
                    match parent {
                        Some(parent) => format!("返信 · {}", hhmm(parent.created_at())),
                        None => "返信".to_owned(),
                    },
                    9.5,
                    TEXT_MUTED,
                ))
                .child(match parent {
                    Some(parent) => ui_text(preview(&parent.body, 48, 2).0, 11.5, TEXT_SECONDARY),
                    None => ui_text("返信先は見つかりません", 11.5, TEXT_MUTED),
                }),
        );
    }
    if capture.session_open {
        body = body.child(
            div()
                .h(32.0_f32)
                .w_full()
                .rounded(7.0_f32)
                .bg(rgb(0x241e31))
                .px(12.0_f32)
                .flex()
                .flex_row()
                .items_center()
                .gap(6.0_f32)
                .child(div().w(5.0_f32).h(5.0_f32).rounded(3.0_f32).bg(rgb(ACCENT)))
                .child(ui_text("進行中", 10.5, TEXT_SECONDARY)),
        );
    }
    for paragraph in capture.body.split("\n\n") {
        let paragraph = paragraph.trim().replace('\n', " ");
        if paragraph.is_empty() {
            continue;
        }
        body = body.child(
            ui_text(paragraph, 15.0, TEXT)
                .line_height(LineHeight::Px(26.0_f32))
                .w_full(),
        );
    }
    let meta = format!(
        "capture · {} · 作成 {}",
        capture.short_id(),
        hhmm(capture.created_at())
    );

    div()
        .w(DETAIL_W)
        .h(DETAIL_H)
        .rounded(18.0_f32)
        .bg(rgb(PANEL))
        .border(1.0_f32)
        .border_color(rgb(BORDER))
        .shadow(BoxShadow {
            offset_y: 24.0,
            blur: 64.0,
            color: Color::from_rgba(0x05030888),
            ..Default::default()
        })
        .overflow_hidden()
        .flex()
        .flex_col()
        .relative()
        .child(
            div()
                .h(56.0_f32)
                .w_full()
                .flex_none()
                .px(24.0_f32)
                .flex()
                .flex_row()
                .items_center()
                .gap(8.0_f32)
                .child(
                    div()
                        .cursor(CursorIcon::Pointer)
                        .on_click(move |_| {
                            ui.detail.set(None);
                        })
                        .child(icon(&icons::ARROW_BACK, 16.0_f32).text_color(rgb(TEXT_SECONDARY))),
                )
                .child(
                    text(move || ui.filter.get().label().to_owned())
                        .font_family(SANS)
                        .text_size(12.0_f32)
                        .text_color(rgb(TEXT_SECONDARY)),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .cursor(CursorIcon::Pointer)
                        .on_click(move |_| {
                            ui.command.set(true);
                            ui.command_query.set(String::new());
                            ui.command_selected.set(0);
                            let search = ui.command_search;
                            request_frame(move |_| search.focus());
                        })
                        .child(icon(&icons::MORE_HORIZ, 16.0_f32).text_color(rgb(TEXT_MUTED))),
                ),
        )
        .child(body)
        .child(div().h(1.0_f32).w_full().flex_none().bg(rgb(BORDER)))
        .child(
            div()
                .h(28.0_f32)
                .w_full()
                .flex_none()
                .px(24.0_f32)
                .flex()
                .items_center()
                .child(mono_text(meta, 10.5, TEXT_MUTED)),
        )
        .child(
            div()
                .h(64.0_f32)
                .w_full()
                .flex_none()
                .px(24.0_f32)
                .flex()
                .flex_row()
                .items_center()
                .gap(10.0_f32)
                .child({
                    let ui = ui;
                    let id = capture.id.clone();
                    div()
                        .h(30.0_f32)
                        .px(11.0_f32)
                        .rounded(7.0_f32)
                        .flex()
                        .items_center()
                        .gap(6.0_f32)
                        .cursor(CursorIcon::Pointer)
                        .on_click(move |_| ui.reply.set(Some(id.clone())))
                        .child(ui_text("返信", 11.5, TEXT_SECONDARY))
                        .child(keycap("r"))
                })
                .child({
                    let session = session.clone();
                    let id = capture.id.clone();
                    let archived = capture.archived;
                    let ui = ui;
                    div()
                        .h(30.0_f32)
                        .px(11.0_f32)
                        .rounded(7.0_f32)
                        .flex()
                        .items_center()
                        .gap(6.0_f32)
                        .cursor(CursorIcon::Pointer)
                        .on_click(move |_| {
                            if archived {
                                session.unarchive(&id);
                            } else {
                                session.archive(&id);
                                ui.detail.set(None);
                            }
                        })
                        .child(ui_text(
                            if archived {
                                "戻す"
                            } else {
                                "アーカイブ"
                            },
                            11.5,
                            TEXT_SECONDARY,
                        ))
                        .child(keycap(if archived { "u" } else { "a" }))
                })
                .child({
                    let session = session.clone();
                    let id = capture.id.clone();
                    let pinned = capture.pinned;
                    div()
                        .h(30.0_f32)
                        .px(11.0_f32)
                        .rounded(7.0_f32)
                        .flex()
                        .items_center()
                        .cursor(CursorIcon::Pointer)
                        .on_click(move |_| session.pin(&id, !pinned))
                        .gap(6.0_f32)
                        .child(ui_text(
                            if pinned { "ピン解除" } else { "ピン" },
                            11.5,
                            TEXT_SECONDARY,
                        ))
                        .child(keycap("p"))
                })
                .child({
                    let text = copy_body(capture);
                    div()
                        .h(30.0_f32)
                        .px(11.0_f32)
                        .rounded(7.0_f32)
                        .flex()
                        .items_center()
                        .cursor(CursorIcon::Pointer)
                        .on_click(move |_| copy_text(text.clone()))
                        .gap(6.0_f32)
                        .child(ui_text("コピー", 11.5, TEXT_SECONDARY))
                        .child(keycap("c"))
                })
                .child(div().flex_1())
                .child(ui_text("h / Esc 戻る", 10.5, TEXT_MUTED)),
        )
        .child(hud_overlays(session, ui, layer.clone()))
}

/// Copy through `wl-copy` so the selection outlives the HUD layer. The
/// in-process data source dies when the surface closes, which made a copy
/// disappear unless the clipboard manager had already recorded it.
fn copy_text(text: String) {
    let copied = match Command::new("wl-copy")
        .arg("--type")
        .arg("text/plain;charset=utf-8")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(mut child) => {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            child.wait().map(|status| status.success()).unwrap_or(false)
        }
        Err(_) => false,
    };
    if !copied {
        set_clipboard(ClipboardKind::Clipboard, text);
    }
}

fn copy_body(capture: &Capture) -> String {
    capture.body.clone()
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StoredView {
    version: u8,
    last_seen_micros: i64,
}

fn view_path() -> Option<std::path::PathBuf> {
    let mut path = mote::state_dir().ok()?;
    std::fs::create_dir_all(&path).ok()?;
    path.push("hud-view.json");
    Some(path)
}

fn load_last_seen() -> i64 {
    let Some(path) = view_path() else {
        return 0;
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return 0;
    };
    serde_json::from_slice::<StoredView>(&bytes)
        .ok()
        .filter(|view| view.version == 1)
        .map(|view| view.last_seen_micros)
        .unwrap_or(0)
}

/// Returns the previous seen time and records now as seen.
fn mark_last_seen() -> i64 {
    let previous = load_last_seen();
    let now = now_micros().unwrap_or(0);
    if let Some(path) = view_path() {
        let view = StoredView {
            version: 1,
            last_seen_micros: now,
        };
        if let Ok(bytes) = serde_json::to_vec(&view) {
            let temporary = path.with_extension("tmp");
            let _ = std::fs::write(&temporary, bytes);
            let _ = std::fs::rename(&temporary, &path);
        }
    }
    previous
}

fn bench_model(count: usize) -> Model {
    let fixture = fixture_model();
    let base = fixture
        .captures
        .first()
        .cloned()
        .expect("the fixture has at least one capture");
    let now = now_micros().unwrap_or(0);
    let mut model = fixture;
    model.captures.clear();
    for index in 0..count {
        let mut capture = base.clone();
        capture.id = format!("{:064x}", index + 1);
        capture.body = format!(
            "bench capture {index} 本文は表示幅の確認用に少し長くしておく。二行目にもなる。"
        );
        capture.created_at_micros = Some(now - index as i64 * 60_000_000);
        model.captures.push(capture);
    }
    model
}

fn bench_list(count: usize) {
    let model = bench_model(count);
    let ui = UiState::new(0);
    ui.scroll_y.set(4000.0);
    let started = Instant::now();
    let width = SURFACE_W as u32;
    let height = SURFACE_H as u32;
    anti_webui::app::headless::render_png("target/hud-bench.png", width, height, 1.0, || {
        surface(static_panel(&model, ui))
    })
    .expect("the benchmark render");
    println!("{count} captures rendered in {:?}", started.elapsed());
}

// ---------------------------------------------------------------------------
// Fixture for `--snapshot`
// ---------------------------------------------------------------------------

fn fixture_model() -> Model {
    let views = vec![
        fixture_view(
            0x11,
            "明日の打ち合わせ、資料は共有済み。先方の希望で14時からに変更。",
            "2026-11-27 12:31",
            false,
            None,
            false,
        ),
        fixture_view(
            0x22,
            "購買リストの件、Wi-Fiルーターの型番を確認する。旧宅の回線はまだ生きているので、解約のタイミングも決める。候補は二つ。Aは6E対応、Bは7対応で少し高い。部屋の広さなら6Eで足りそうだが…",
            "2026-11-27 11:58",
            false,
            None,
            true,
        ),
        fixture_view(
            0x33,
            "購買リストのレビュー、木曜の午前に入れる。旧宅の鍵の返却、回線の解約、ルーターの設置場所を同じ日にまとめる。立ち会いが必要なら平日。月末は混むので避ける。候補は A: XX-6E / B: XX-7。",
            "2026-11-27 10:20",
            false,
            None,
            false,
        ),
        fixture_view(
            0x44,
            "429のときのリトライ間隔、指数バックオフでよさそう。",
            "2026-11-27 09:12",
            false,
            Some("11".repeat(32).as_str()),
            false,
        ),
        fixture_view(
            0x55,
            "Moteデスクトップの一覧性、吹き出しをやめて時刻を固定する案。",
            "2026-11-27 08:40",
            true,
            None,
            false,
        ),
        fixture_view(
            0x66,
            "Rustのnon_exhaustive、外部crateのenumに付けると…",
            "2026-11-26 23:40",
            false,
            None,
            false,
        ),
        fixture_view(
            0x77,
            "音声入力のテストメモ。静かな部屋では十分だけど、歩きながらだと固有名詞が弱い。…",
            "2026-11-26 22:10",
            false,
            None,
            false,
        ),
        fixture_view(
            0x88,
            "オフラインで書いたメモ。接続したら送る。",
            "2026-11-26 19:42",
            false,
            None,
            false,
        ),
    ];
    let captures = views
        .iter()
        .filter_map(|view| Capture::from_view(view).ok())
        .collect();
    Model {
        captures,
        pending: Vec::new(),
        status: Status::Connected,
        last_sync: Some(now_micros().unwrap_or(0)),
        error: None,
        from_cache: false,
    }
}

fn fixture_view(
    seed: u8,
    body: &str,
    at: &str,
    pinned: bool,
    reply_to: Option<&str>,
    session_open: bool,
) -> serde_json::Value {
    let at = fixture_time(at);
    let mut attributes = vec![json!({
        "namespace": "capture",
        "key": "created_at_micros",
        "value": at,
    })];
    if pinned {
        attributes.push(json!({"namespace": "capture", "key": "pinned", "value": true}));
    }
    if let Some(reply_to) = reply_to {
        attributes.push(json!({"namespace": "capture", "key": "reply_to", "value": reply_to}));
    }
    if session_open {
        attributes.push(json!({"namespace": "capture", "key": "session_open", "value": true}));
    }
    json!({
        "content": {
            "id": format!("{seed:02x}{:062x}", 0),
            "body": body,
            "kinds": ["capture"],
            "attributes": attributes,
        },
        "conflicts": [],
        "deletion_conflict": false,
    })
}

fn fixture_time(value: &str) -> i64 {
    let parsed = time::PrimitiveDateTime::parse(
        value,
        time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]"),
    )
    .expect("the fixture time");
    parsed
        .assume_offset(UtcOffset::from_hms(9, 0, 0).unwrap())
        .unix_timestamp()
        * 1_000_000
}

fn local_at(micros: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(micros.div_euclid(1_000_000))
        .unwrap_or(OffsetDateTime::UNIX_EPOCH)
        .to_offset(UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC))
}

fn hhmm(micros: i64) -> String {
    let at = local_at(micros);
    format!("{:02}:{:02}", at.hour(), at.minute())
}

fn date_label(at: OffsetDateTime) -> String {
    let weekday = match at.weekday() {
        Weekday::Monday => "月",
        Weekday::Tuesday => "火",
        Weekday::Wednesday => "水",
        Weekday::Thursday => "木",
        Weekday::Friday => "金",
        Weekday::Saturday => "土",
        Weekday::Sunday => "日",
    };
    format!("{}月{}日（{weekday}）", at.month() as u8, at.day())
}

// ---------------------------------------------------------------------------
// Capture
// ---------------------------------------------------------------------------

fn capture_panel(session: &Session) -> Div {
    compose_sheet("新しいCapture", None, None, session, close_window)
}

fn compose_sheet(
    title: &'static str,
    quote: Option<String>,
    reply_to: Option<String>,
    session: &Session,
    on_close: impl Fn() + 'static,
) -> Div {
    let area = node_ref();
    let text = signal(String::new());
    let error = signal(Option::<String>::None);
    request_frame(move |_| area.focus());
    let on_close: Rc<dyn Fn()> = Rc::new(on_close);

    let save: Rc<dyn Fn()> = Rc::new({
        let session = session.clone();
        let reply_to = reply_to.clone();
        let on_close = on_close.clone();
        move || {
            let body = text.get();
            match spool_capture(&body, reply_to.as_deref()) {
                Ok(()) => {
                    session.retry();
                    on_close();
                }
                Err(message) => error.set(Some(message)),
            }
        }
    });
    let submit = save.clone();
    let click = save.clone();
    let close = on_close.clone();

    let mut panel = div()
        .w(CAPTURE_W)
        .h(CAPTURE_H)
        .rounded(16.0_f32)
        .bg(rgb(PANEL))
        .border(1.0_f32)
        .border_color(rgb(BORDER))
        .shadow(BoxShadow {
            offset_y: 18.0,
            blur: 48.0,
            color: Color::from_rgba(0x05030888),
            ..Default::default()
        })
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(
            div()
                .h(56.0_f32)
                .flex_none()
                .px(24.0_f32)
                .flex()
                .flex_row()
                .items_center()
                .child(ui_text(title, 13.0, TEXT).font_weight(FontWeight::SEMIBOLD))
                .child(div().flex_1())
                .child(
                    div()
                        .cursor(CursorIcon::Pointer)
                        .on_click({
                            let close = close.clone();
                            move |_| close()
                        })
                        .child(icon(&icons::CLOSE, 16.0_f32).text_color(rgb(TEXT_MUTED))),
                ),
        );
    if let Some(quote) = quote {
        panel = panel.child(
            div()
                .mx(24.0_f32)
                .mb(8.0_f32)
                .rounded(7.0_f32)
                .bg(rgb(0x1d1927))
                .px(12.0_f32)
                .py(8.0_f32)
                .child(
                    ui_text(quote, 11.0, TEXT_MUTED)
                        .line_height(LineHeight::Px(17.0_f32))
                        .w_full(),
                ),
        );
    }
    panel
        .child(
            div()
                .flex_1()
                .min_h(0.0_f32)
                .px(24.0_f32)
                .py(12.0_f32)
                .child(
                    text_area()
                        .node_ref(area)
                        .value(text)
                        .rows(8)
                        .on_submit(move |_| submit())
                        .font_family(SANS)
                        .text_size(15.0_f32)
                        .line_height(LineHeight::Px(26.0))
                        .text_color(rgb(TEXT))
                        .w_full()
                        .h_full(),
                ),
        )
        .child(
            div()
                .h(64.0_f32)
                .flex_none()
                .px(24.0_f32)
                .flex()
                .flex_row()
                .items_center()
                .gap(10.0_f32)
                .child(dyn_view(move || match error.get() {
                    Some(message) => ui_text(message, 10.5, WARNING),
                    None => ui_text("", 10.5, WARNING),
                }))
                .child(div().flex_1())
                .child(mono_text("Ctrl+Enter", 10.5, TEXT_MUTED))
                .child(
                    div()
                        .h(32.0_f32)
                        .px(20.0_f32)
                        .rounded(8.0_f32)
                        .bg(rgb(ACCENT))
                        .flex()
                        .items_center()
                        .cursor(CursorIcon::Pointer)
                        .on_click(move |_| click())
                        .child(ui_text("保存", 11.5, ON_ACCENT).font_weight(FontWeight::SEMIBOLD)),
                ),
        )
}

fn spool_capture(body: &str, reply_to: Option<&str>) -> Result<(), String> {
    let body = body.trim().to_owned();
    if body.is_empty() {
        return Err("本文が空です".into());
    }
    if body.len() > mote::MAX_CAPTURE_BYTES {
        return Err("長すぎます".into());
    }
    let spool = Spool::open().map_err(|error| error.to_string())?;
    let pending = PendingCapture::new(body, reply_to.map(str::to_owned))
        .map_err(|error| error.to_string())?;
    spool.save(&pending).map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_wraps_but_does_not_mark_a_short_overflow_as_truncated() {
        let body = "あ".repeat(40);
        let (_, lines, truncated) = preview(&body, 34, 3);
        assert_eq!(lines, 2);
        assert!(!truncated);
    }

    #[test]
    fn preview_marks_three_line_clamp_as_truncated() {
        let body = "あ".repeat(110);
        let (preview, lines, truncated) = preview(&body, 34, 3);
        assert_eq!(lines, 3);
        assert!(truncated);
        assert!(preview.ends_with('…'));
    }

    #[test]
    fn only_truncated_rows_show_a_character_count() {
        let short = "あ".repeat(40);
        let long = "あ".repeat(110);
        let short_entry = Entry {
            kind: EntryKind::Pending,
            body: &short,
            micros: 0,
        };
        let long_entry = Entry {
            kind: EntryKind::Pending,
            body: &long,
            micros: 0,
        };
        assert!(row_of(&short_entry, false, None).trailer.is_none());
        assert_eq!(
            row_of(&long_entry, false, None).trailer.as_deref(),
            Some("110字")
        );
    }

    #[test]
    fn error_labels_name_the_next_step() {
        assert_eq!(
            error_label("OAuth refresh failed: invalid_grant"),
            "mote connect で再接続"
        );
        assert_eq!(
            error_label("Io(Os { code: 111, kind: ConnectionRefused })"),
            "ノードに接続できません"
        );
        assert_eq!(error_label("unexpected"), "操作できませんでした");
    }

    #[test]
    fn grouped_adds_thousands_separators() {
        assert_eq!(grouped(2340), "2,340");
        assert_eq!(grouped(248), "248");
        assert_eq!(grouped(0), "0");
    }

    #[test]
    fn filter_cycle_and_counts() {
        assert_eq!(FilterMode::Inbox.next(), FilterMode::All);
        assert_eq!(FilterMode::Archived.next(), FilterMode::Inbox);
        let model = fixture_model();
        assert_eq!(filter_count(&model, FilterMode::Inbox), model.inbox_len());
        assert!(filter_count(&model, FilterMode::All) >= filter_count(&model, FilterMode::Inbox));
        assert_eq!(
            filter_count(&model, FilterMode::Archived),
            model
                .captures
                .iter()
                .filter(|capture| capture.archived)
                .count()
        );
    }
}
