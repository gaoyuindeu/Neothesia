//! The main window: an activity bar and a library of songs on the left, the player on
//! the right (like an editor with its file explorer)

mod library;

use std::{
    path::{Path, PathBuf},
    rc::Rc,
    sync::mpsc,
    time::{Duration, Instant},
};

use midi_file::midly::MidiMessage;
use nuon::TextAlign;
use winit::{
    event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent},
    keyboard::{Key, NamedKey},
};

use crate::{
    context::Context,
    scene::{
        NuonRenderer, Scene, WindowGfx,
        freeplay::FreeplayScene,
        menu_scene::{self, MenuScene, Page, UiState},
        playing_scene::PlayingScene,
        render_nuon,
    },
    song::Song,
    utils::{BoxFuture, noop_waker_ref},
};

use library::{Library, Row};

const ACTIVITY_W: f32 = 46.0;
const HEADER_H: f32 = 36.0;
const STATUS_H: f32 = 26.0;
const ROW_H: f32 = 24.0;
const INDENT: f32 = 14.0;
const MIN_PANEL_W: f32 = 180.0;
const DOUBLE_CLICK: Duration = Duration::from_millis(450);

const ACTIVITY_BG: [u8; 3] = [21, 20, 25];
const PANEL_BG: [u8; 3] = [29, 27, 34];
const BORDER: [u8; 3] = [42, 40, 50];
const TEXT: [u8; 3] = [214, 210, 224];
const TEXT_DIM: [u8; 3] = [140, 134, 156];
const ICON: [u8; 3] = [128, 122, 144];
const ACCENT: [u8; 3] = [160, 124, 255];
const HOVER: [u8; 3] = [40, 38, 49];
const SELECTED: [u8; 3] = [52, 47, 70];
const SELECTED_FOCUSED: [u8; 3] = [66, 56, 104];
const ERROR: [u8; 3] = [255, 120, 120];

mod icon {
    pub const LIBRARY: &str = "\u{F3C2}";
    pub const TRACKS: &str = "\u{F49F}";
    pub const KEYBOARD: &str = "\u{F451}";
    pub const SETTINGS: &str = "\u{F3E5}";
    pub const CHEVRON_RIGHT: &str = "\u{F285}";
    pub const CHEVRON_DOWN: &str = "\u{F282}";
    pub const MIDI: &str = "\u{F49E}";
    pub const SCORE: &str = "\u{F377}";
    pub const FOLDER_PLUS: &str = "\u{F3D3}";
    pub const REFRESH: &str = "\u{F116}";
    pub const COLLAPSE: &str = "\u{F14B}";
    pub const CLOSE: &str = "\u{F659}";
}

/// What the right side shows
enum MainView {
    Freeplay(Box<FreeplayScene>),
    Playing(Box<PlayingScene>),
    Menu(Box<MenuScene>, Page),
}

impl MainView {
    fn scene(&mut self) -> &mut dyn Scene {
        match self {
            MainView::Freeplay(s) => s.as_mut(),
            MainView::Playing(s) => s.as_mut(),
            MainView::Menu(s, _) => s.as_mut(),
        }
    }

    fn page(&self) -> Option<Page> {
        match self {
            MainView::Menu(_, page) => Some(*page),
            _ => None,
        }
    }
}

/// A song file being read on another thread
struct Loading {
    path: PathBuf,
    rx: mpsc::Receiver<Result<midi_file::MidiFile, String>>,
}

enum Status {
    None,
    Info(String),
    Error(String),
}

/// Things the side bar asked for this frame
enum Action {
    Click(PathBuf, bool),
    RemoveRoot(PathBuf),
    AddFolder,
    Refresh,
    CollapseAll,
    ToggleLibrary,
    StartResize(f32),
    Page(Page),
    Freeplay,
}

pub struct Workspace {
    main: MainView,
    song: Option<Song>,
    song_path: Option<PathBuf>,

    library: Library,
    rows: Vec<Row>,
    selected: Option<PathBuf>,
    last_click: Option<(PathBuf, Instant)>,
    /// Keyboard goes to the tree (the last click was in the side bar)
    library_focused: bool,

    panel_visible: bool,
    panel_w: f32,
    /// The left button went down in the side bar: the main view doesn't get it
    sidebar_press: bool,
    divider_drag: Option<f32>,

    loading: Option<Loading>,
    status: Status,
    folder_picker: Option<BoxFuture<Option<PathBuf>>>,

    gfx: Rc<WindowGfx>,
    nuon: nuon::Ui,
    nuon_renderer: NuonRenderer,

    /// Debugging aid (`NEOTHESIA_START_PAGE=tracks|settings`): open this page once the
    /// first song is loaded, for frame dumps of the pages
    start_page: Option<Page>,

    /// Physical rectangles for rendering: main view and the whole window
    view_px: [u32; 4],
    window_px: [u32; 2],
}

impl Workspace {
    pub fn new(ctx: &mut Context) -> Self {
        let folders = if ctx.config.library_folders().is_empty() {
            library::default_folders(ctx.config.last_opened_song().map(PathBuf::as_path))
        } else {
            ctx.config.library_folders().to_vec()
        };
        let library = Library::new(&folders);

        let gfx = Rc::new(WindowGfx::new(ctx));
        let nuon_renderer = NuonRenderer::for_window(ctx, gfx.clone());

        connect_io(ctx);
        let main = MainView::Freeplay(Box::new(FreeplayScene::new(ctx, None)));

        let mut this = Self {
            main,
            song: None,
            song_path: None,
            rows: Vec::new(),
            library,
            selected: None,
            last_click: None,
            library_focused: false,
            panel_visible: ctx.config.sidebar_visible(),
            panel_w: ctx.config.sidebar_width().max(MIN_PANEL_W),
            sidebar_press: false,
            divider_drag: None,
            loading: None,
            status: Status::None,
            folder_picker: None,
            gfx,
            nuon: nuon::Ui::new(),
            nuon_renderer,
            start_page: match std::env::var("NEOTHESIA_START_PAGE").as_deref() {
                Ok("tracks") => Some(Page::TrackSelection),
                Ok("settings") => Some(Page::Settings),
                _ => None,
            },
            view_px: [0; 4],
            window_px: [1, 1],
        };
        this.layout(ctx);

        // The file from the command line, or the last one
        let start = std::env::args()
            .nth(1)
            .map(PathBuf::from)
            .or_else(|| ctx.config.last_opened_song().cloned())
            .filter(|p| p.exists());
        if let Some(path) = start {
            this.library.reveal(&path);
            this.selected = Some(path.clone());
            this.load(path);
        }
        this
    }

    fn sidebar_w(&self) -> f32 {
        ACTIVITY_W
            + if self.panel_visible {
                self.panel_w
            } else {
                0.0
            }
    }

    /// Keep the main view next to the side bar
    fn layout(&mut self, ctx: &mut Context) {
        let max_panel = (ctx.full_window_state.logical_size.width * 0.6).max(MIN_PANEL_W);
        self.panel_w = self.panel_w.clamp(MIN_PANEL_W, max_panel);

        let left = self.sidebar_w();
        if ctx.view_left() != left {
            ctx.set_view_left(left);
            let size = ctx.window_state.physical_size;
            self.main
                .scene()
                .window_event(ctx, &WindowEvent::Resized(size));
        }

        let full = ctx.full_window_state.physical_size;
        let x = ctx.view_left_px();
        self.view_px = [
            x,
            0,
            full.width.saturating_sub(x).max(1),
            full.height.max(1),
        ];
        self.window_px = [full.width.max(1), full.height.max(1)];
    }

    /// Read a song file on another thread; it opens when ready
    fn load(&mut self, path: PathBuf) {
        if self.loading.as_ref().is_some_and(|l| l.path == path) {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let file = path.clone();
        std::thread::Builder::new()
            .name("song-loader".into())
            .spawn(move || {
                tx.send(midi_file::MidiFile::new(&file)).ok();
            })
            .ok();
        self.status = Status::Info(format!("Loading {}\u{2026}", file_name(&path)));
        self.loading = Some(Loading { path, rx });
    }

    fn poll_loading(&mut self, ctx: &mut Context) {
        let Some(loading) = &self.loading else {
            return;
        };
        let result = match loading.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("loader stopped".into()),
        };
        let path = self.loading.take().unwrap().path;
        match result {
            Ok(file) => {
                ctx.config.set_last_opened_song(Some(path.clone()));
                self.song_path = Some(path.clone());
                self.status = Status::Info("Space: play / pause \u{2003} Tab: step mode".into());
                ctx.window.set_title(&format!(
                    "{} \u{2014} Neothesia",
                    path.file_stem().unwrap_or_default().to_string_lossy()
                ));
                self.open_song(ctx, Song::new(file), false);
                // Keys go to the player now (arrows step / rewind)
                self.library_focused = false;
                if let Some(page) = self.start_page.take() {
                    self.toggle_page(ctx, page);
                }
            }
            Err(err) => {
                log::error!("{}: {err}", path.display());
                self.status = Status::Error(format!("{}: {err}", file_name(&path)));
            }
        }
    }

    /// Show `song` in the player, playing or paused at the start
    fn open_song(&mut self, ctx: &mut Context, song: Song, play: bool) {
        connect_io(ctx);
        let mut scene = PlayingScene::new(ctx, song.clone());
        if !play {
            scene.pause();
        }
        self.song = Some(song);
        self.main = MainView::Playing(Box::new(scene));
    }

    fn show_freeplay(&mut self, ctx: &mut Context) {
        connect_io(ctx);
        let scene = FreeplayScene::new(ctx, self.song.clone());
        self.main = MainView::Freeplay(Box::new(scene));
    }

    /// Settings or the track page on the right; the same button closes it
    fn toggle_page(&mut self, ctx: &mut Context, page: Page) {
        if self.main.page() == Some(page) {
            self.close_menu(ctx, self.song.clone());
            return;
        }
        if page == Page::TrackSelection && self.song.is_none() {
            self.status = Status::Info("Open a song first".into());
            return;
        }
        let menu = MenuScene::embedded(ctx, self.song.clone(), page);
        self.main = MainView::Menu(Box::new(menu), page);
    }

    fn close_menu(&mut self, ctx: &mut Context, song: Option<Song>) {
        match song {
            Some(song) => self.open_song(ctx, song, false),
            None => self.show_freeplay(ctx),
        }
    }

    fn add_folder(&mut self, ctx: &mut Context, path: PathBuf) {
        if self.library.add_root(path) {
            ctx.config.set_library_folders(self.library.root_paths());
        }
    }

    // Events from the scenes

    /// A song was chosen to play (track page)
    pub fn play(&mut self, ctx: &mut Context, song: Song) {
        self.open_song(ctx, song, true);
    }

    pub fn freeplay(&mut self, ctx: &mut Context, song: Option<Song>) {
        if song.is_some() {
            self.song = song;
        }
        self.show_freeplay(ctx);
    }

    /// Back / Esc / the song ended: show the library
    pub fn back(&mut self, ctx: &mut Context, song: Option<Song>) {
        match &self.main {
            MainView::Freeplay(_) if song.is_some() || self.song.is_some() => {
                let song = song.or_else(|| self.song.clone()).unwrap();
                self.open_song(ctx, song, false);
            }
            MainView::Menu(..) => self.close_menu(ctx, song),
            _ => {
                self.panel_visible = true;
                self.library_focused = true;
            }
        }
    }

    pub fn menu_closed(&mut self, ctx: &mut Context, song: Option<Song>) {
        self.close_menu(ctx, song.or_else(|| self.song.clone()));
    }

    pub fn midi_event(&mut self, ctx: &mut Context, channel: u8, message: &MidiMessage) {
        self.main.scene().midi_event(ctx, channel, message);
    }

    pub fn update(&mut self, ctx: &mut Context, delta: Duration) {
        self.poll_loading(ctx);

        if let Some(picker) = self.folder_picker.as_mut() {
            let mut cx = std::task::Context::from_waker(noop_waker_ref());
            if let std::task::Poll::Ready(folder) = picker.as_mut().poll(&mut cx) {
                self.folder_picker = None;
                if let Some(folder) = folder {
                    self.add_folder(ctx, folder);
                }
            }
        }

        self.rows = self.library.rows();
        let actions = self.sidebar_ui(ctx);
        for action in actions {
            self.apply(ctx, action);
        }

        if let Some(offset) = self.divider_drag {
            let x = ctx.full_window_state.cursor_logical_position.x;
            self.panel_w = x - ACTIVITY_W - offset;
        }
        self.layout(ctx);
        ctx.config.set_sidebar_visible(self.panel_visible);
        ctx.config.set_sidebar_width(self.panel_w);

        self.main.scene().update(ctx, delta);

        self.gfx.update(ctx);
        render_nuon(&mut self.nuon, &mut self.nuon_renderer, ctx);
    }

    pub fn render<'pass>(&'pass mut self, rpass: &mut wgpu_jumpstart::RenderPass<'pass>) {
        let [x, y, w, h] = self.view_px;
        rpass.set_view(x, y, w, h);
        self.main.scene().render(rpass);

        let [w, h] = self.window_px;
        rpass.set_view(0, 0, w, h);
        self.nuon_renderer.render(rpass);
    }

    pub fn end_frame(&mut self) {
        self.gfx.text_factory.trim();
    }

    fn apply(&mut self, ctx: &mut Context, action: Action) {
        match action {
            Action::Click(path, is_dir) => {
                self.library_focused = true;
                let double = self
                    .last_click
                    .as_ref()
                    .is_some_and(|(p, t)| *p == path && t.elapsed() < DOUBLE_CLICK);
                self.selected = Some(path.clone());
                if is_dir {
                    self.library.toggle(&path);
                    self.last_click = None;
                } else if double {
                    self.last_click = None;
                    self.load(path);
                } else {
                    self.last_click = Some((path, Instant::now()));
                }
            }
            Action::RemoveRoot(path) => {
                self.library.remove_root(&path);
                ctx.config.set_library_folders(self.library.root_paths());
            }
            Action::AddFolder => {
                if self.folder_picker.is_none() {
                    self.folder_picker = Some(Box::pin(async {
                        rfd::AsyncFileDialog::new()
                            .pick_folder()
                            .await
                            .map(|f| f.path().to_path_buf())
                    }));
                }
            }
            Action::Refresh => self.library.refresh(),
            Action::CollapseAll => self.library.collapse_all(),
            Action::ToggleLibrary => {
                self.panel_visible = !self.panel_visible;
                self.library_focused = self.panel_visible;
            }
            Action::StartResize(offset) => self.divider_drag = Some(offset),
            Action::Page(page) => self.toggle_page(ctx, page),
            Action::Freeplay => match self.main {
                MainView::Freeplay(_) => self.back(ctx, None),
                _ => self.show_freeplay(ctx),
            },
        }
    }

    // Input

    pub fn window_event(&mut self, ctx: &mut Context, event: &WindowEvent) {
        let cursor = ctx.full_window_state.cursor_logical_position;
        let in_sidebar = cursor.x >= 0.0 && cursor.x < self.sidebar_w();

        match event {
            WindowEvent::CursorMoved { .. } => {
                self.nuon.mouse_move(cursor.x, cursor.y);
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } if in_sidebar => {
                self.sidebar_press = true;
                self.library_focused = self.panel_visible;
                self.nuon.mouse_down();
                return;
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => {
                self.nuon.mouse_up();
                self.divider_drag = None;
                if std::mem::take(&mut self.sidebar_press) {
                    return;
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                ..
            } => {
                if in_sidebar {
                    return;
                }
                self.library_focused = false;
            }
            WindowEvent::MouseWheel { delta, .. } if in_sidebar => {
                let y = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y * 60.0,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32,
                };
                self.nuon.mouse_wheel(y);
                return;
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ElementState::Pressed,
                        logical_key,
                        ..
                    },
                ..
            } => {
                let ctrl = ctx.full_window_state.modifiers_state.control_key();
                if ctrl && matches!(logical_key, Key::Character(c) if c.eq_ignore_ascii_case("b")) {
                    self.panel_visible = !self.panel_visible;
                    self.library_focused = self.panel_visible;
                    return;
                }
                if self.library_focused && self.panel_visible && self.tree_key(logical_key) {
                    return;
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ElementState::Released,
                        logical_key,
                        ..
                    },
                ..
            } if self.library_focused && self.panel_visible && is_tree_key(logical_key) => {
                return;
            }
            WindowEvent::DroppedFile(path) => {
                if path.is_dir() {
                    self.add_folder(ctx, path.clone());
                } else if library::is_song_file(path) {
                    self.selected = Some(path.clone());
                    self.load(path.clone());
                }
                return;
            }
            _ => {}
        }

        self.main.scene().window_event(ctx, event);
    }

    /// Arrow keys and Enter in the tree; true when used
    fn tree_key(&mut self, key: &Key) -> bool {
        if !is_tree_key(key) {
            return false;
        }
        let rows = self.library.rows();
        if rows.is_empty() {
            return true;
        }
        let pos = self
            .selected
            .as_ref()
            .and_then(|s| rows.iter().position(|r| &r.path == s));
        let Some(i) = pos else {
            self.selected = Some(rows[0].path.clone());
            return true;
        };
        let row = &rows[i];
        match key {
            Key::Named(NamedKey::ArrowUp) => {
                self.selected = Some(rows[i.saturating_sub(1)].path.clone());
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.selected = Some(rows[(i + 1).min(rows.len() - 1)].path.clone());
            }
            Key::Named(NamedKey::ArrowRight) if row.is_dir => {
                if row.expanded {
                    if let Some(next) = rows.get(i + 1).filter(|r| r.depth > row.depth) {
                        self.selected = Some(next.path.clone());
                    }
                } else {
                    self.library.set_expanded(&row.path, true);
                }
            }
            Key::Named(NamedKey::ArrowLeft) => {
                if row.is_dir && row.expanded {
                    self.library.set_expanded(&row.path, false);
                } else if let Some(parent) = rows[..i].iter().rev().find(|r| r.depth < row.depth) {
                    self.selected = Some(parent.path.clone());
                }
            }
            Key::Named(NamedKey::Enter) => {
                if row.is_dir {
                    self.library.toggle(&row.path);
                } else {
                    self.load(row.path.clone());
                }
            }
            _ => {}
        }
        true
    }

    // Drawing

    fn sidebar_ui(&mut self, ctx: &Context) -> Vec<Action> {
        let mut actions = Vec::new();
        let h = ctx.full_window_state.logical_size.height;
        let mut ui = std::mem::replace(&mut self.nuon, nuon::Ui::new());

        self.activity_bar(&mut ui, h, &mut actions);
        if self.panel_visible {
            nuon::translate().x(ACTIVITY_W).build(&mut ui, |ui| {
                self.panel(ui, h, &mut actions);
            });
        }

        self.nuon = ui;
        actions
    }

    fn activity_bar(&self, ui: &mut nuon::Ui, h: f32, actions: &mut Vec<Action>) {
        nuon::quad()
            .size(ACTIVITY_W, h)
            .color(ACTIVITY_BG)
            .build(ui);

        let page = self.main.page();
        let items = [
            (
                icon::LIBRARY,
                self.panel_visible,
                0.0,
                Action::ToggleLibrary,
            ),
            (
                icon::TRACKS,
                page == Some(Page::TrackSelection),
                ACTIVITY_W,
                Action::Page(Page::TrackSelection),
            ),
            (
                icon::KEYBOARD,
                matches!(self.main, MainView::Freeplay(_)),
                ACTIVITY_W * 2.0,
                Action::Freeplay,
            ),
            (
                icon::SETTINGS,
                page == Some(Page::Settings),
                h - ACTIVITY_W,
                Action::Page(Page::Settings),
            ),
        ];
        for (glyph, active, y, action) in items {
            let ev = nuon::click_area(("activity", glyph))
                .pos(0.0, y)
                .size(ACTIVITY_W, ACTIVITY_W)
                .build(ui);
            if active {
                nuon::quad()
                    .pos(0.0, y + 8.0)
                    .size(2.0, ACTIVITY_W - 16.0)
                    .color(ACCENT)
                    .build(ui);
            }
            let color = if active || ev.is_hovered() || ev.is_pressed() {
                [236, 232, 246]
            } else {
                ICON
            };
            nuon::label()
                .icon(glyph)
                .pos(0.0, y)
                .size(ACTIVITY_W, ACTIVITY_W)
                .font_size(21.0)
                .color(color)
                .text_justify(TextAlign::Center)
                .build(ui);
            if ev.is_clicked() {
                actions.push(action);
            }
        }
    }

    fn panel(&self, ui: &mut nuon::Ui, h: f32, actions: &mut Vec<Action>) {
        let w = self.panel_w;
        nuon::quad().size(w, h).color(PANEL_BG).build(ui);

        // Header
        nuon::label()
            .text("LIBRARY")
            .bold(true)
            .pos(14.0, 0.0)
            .size(w - 100.0, HEADER_H)
            .font_size(11.0)
            .color(TEXT_DIM)
            .text_justify(TextAlign::Start)
            .text_align(TextAlign::Center)
            .build(ui);
        let buttons = [
            (icon::FOLDER_PLUS, Action::AddFolder),
            (icon::REFRESH, Action::Refresh),
            (icon::COLLAPSE, Action::CollapseAll),
        ];
        let btn = 26.0;
        for (i, (glyph, action)) in buttons.into_iter().enumerate() {
            let x = w - 8.0 - btn * (3 - i) as f32;
            let y = (HEADER_H - btn) / 2.0;
            if icon_button(ui, ("panel", glyph), x, y, btn, glyph, 15.0) {
                actions.push(action);
            }
        }

        // Tree
        let tree_h = (h - HEADER_H - STATUS_H).max(0.0);
        nuon::translate().y(HEADER_H).build(ui, |ui| {
            if self.rows.is_empty() {
                self.empty_library(ui, w, actions);
                return;
            }
            nuon::scroll("library_tree")
                .scissor_size(w, tree_h)
                .build(ui, |ui| {
                    for (i, row) in self.rows.iter().enumerate() {
                        self.tree_row(ui, row, i as f32 * ROW_H, w, actions);
                    }
                    // The height of the content
                    nuon::translate()
                        .y(self.rows.len() as f32 * ROW_H + 8.0)
                        .add_to_current(ui);
                });
        });

        // Status line
        nuon::quad()
            .pos(0.0, h - STATUS_H)
            .size(w, 1.0)
            .color(BORDER)
            .build(ui);
        let (text, color) = match &self.status {
            Status::None => (String::new(), TEXT_DIM),
            Status::Info(s) => (s.clone(), TEXT_DIM),
            Status::Error(s) => (s.clone(), ERROR),
        };
        nuon::label()
            .text(ellipsize(&text, w - 24.0, 11.5))
            .pos(12.0, h - STATUS_H)
            .size(w - 24.0, STATUS_H)
            .font_size(11.5)
            .color(color)
            .text_justify(TextAlign::Start)
            .text_align(TextAlign::Center)
            .build(ui);

        // Border and resize handle
        let ev = nuon::click_area("panel_divider")
            .pos(w - 3.0, 0.0)
            .size(6.0, h)
            .build(ui);
        let hot = ev.is_hovered() || ev.is_pressed() || self.divider_drag.is_some();
        nuon::quad()
            .pos(w - 1.0, 0.0)
            .size(if hot { 2.0 } else { 1.0 }, h)
            .color(if hot { ACCENT } else { BORDER })
            .build(ui);
        if ev.is_press_start() {
            // Keep the grabbed point under the cursor
            actions.push(Action::StartResize(ev.local_pos().x - 3.0));
        }
    }

    fn empty_library(&self, ui: &mut nuon::Ui, w: f32, actions: &mut Vec<Action>) {
        nuon::label()
            .text("No folders yet.")
            .pos(16.0, 16.0)
            .size(w - 32.0, 20.0)
            .font_size(13.0)
            .color(TEXT_DIM)
            .text_justify(TextAlign::Start)
            .build(ui);
        nuon::label()
            .text("Add a folder with MIDI or MusicXML files,")
            .pos(16.0, 38.0)
            .size(w - 32.0, 18.0)
            .font_size(12.0)
            .color(TEXT_DIM)
            .text_justify(TextAlign::Start)
            .build(ui);
        nuon::label()
            .text("or drop one on the window.")
            .pos(16.0, 56.0)
            .size(w - 32.0, 18.0)
            .font_size(12.0)
            .color(TEXT_DIM)
            .text_justify(TextAlign::Start)
            .build(ui);
        if nuon::button()
            .id("add_folder_big")
            .pos(16.0, 88.0)
            .size((w - 32.0).min(220.0), 32.0)
            .color([74, 60, 120])
            .hover_color([92, 76, 148])
            .preseed_color([60, 48, 100])
            .border_radius([6.0; 4])
            .label("Add Folder")
            .build(ui)
        {
            actions.push(Action::AddFolder);
        }
    }

    fn tree_row(&self, ui: &mut nuon::Ui, row: &Row, y: f32, w: f32, actions: &mut Vec<Action>) {
        let row_w = w - 12.0;
        let ev = nuon::click_area(("row", &row.path))
            .pos(0.0, y)
            .size(row_w, ROW_H)
            .build(ui);

        let selected = self.selected.as_ref() == Some(&row.path);
        let bg = if selected && self.library_focused {
            Some(SELECTED_FOCUSED)
        } else if selected {
            Some(SELECTED)
        } else if ev.is_hovered() || ev.is_pressed() {
            Some(HOVER)
        } else {
            None
        };
        if let Some(bg) = bg {
            nuon::quad()
                .pos(4.0, y)
                .size(row_w - 4.0, ROW_H)
                .color(bg)
                .border_radius([4.0; 4])
                .build(ui);
        }

        let is_open = self.song_path.as_ref() == Some(&row.path);
        let is_loading = self.loading.as_ref().is_some_and(|l| l.path == row.path);

        let x = 10.0 + row.depth as f32 * INDENT;
        let glyph = if row.is_dir {
            if row.expanded {
                icon::CHEVRON_DOWN
            } else {
                icon::CHEVRON_RIGHT
            }
        } else if is_score(&row.path) {
            icon::SCORE
        } else {
            icon::MIDI
        };
        let icon_color = if is_open {
            ACCENT
        } else if row.is_dir {
            TEXT_DIM
        } else {
            ICON
        };
        nuon::label()
            .icon(glyph)
            .pos(x, y)
            .size(16.0, ROW_H)
            .font_size(if row.is_dir { 11.0 } else { 13.0 })
            .color(icon_color)
            .text_justify(TextAlign::Center)
            .build(ui);

        let text_x = x + 20.0;
        let remove_w = if row.is_root && ev.is_hovered() {
            22.0
        } else {
            0.0
        };
        let text_w = (row_w - text_x - 6.0 - remove_w).max(0.0);
        let name = if is_loading {
            format!("{} \u{2026}", row.name)
        } else {
            row.name.clone()
        };
        nuon::label()
            .text(ellipsize(&name, text_w, 13.0))
            .bold(row.is_root)
            .pos(text_x, y)
            .size(text_w, ROW_H)
            .font_size(13.0)
            .color(if is_open { ACCENT } else { TEXT })
            .text_justify(TextAlign::Start)
            .text_align(TextAlign::Center)
            .build(ui);

        if remove_w > 0.0 {
            let bx = row_w - remove_w - 2.0;
            if icon_button(
                ui,
                ("remove", &row.path),
                bx,
                y + 2.0,
                ROW_H - 4.0,
                icon::CLOSE,
                11.0,
            ) {
                actions.push(Action::RemoveRoot(row.path.clone()));
                return;
            }
        }

        if ev.is_clicked() {
            actions.push(Action::Click(row.path.clone(), row.is_dir));
        }
    }
}

fn icon_button(
    ui: &mut nuon::Ui,
    id: impl std::hash::Hash,
    x: f32,
    y: f32,
    size: f32,
    glyph: &'static str,
    font_size: f32,
) -> bool {
    let ev = nuon::click_area(id).pos(x, y).size(size, size).build(ui);
    if ev.is_hovered() || ev.is_pressed() {
        nuon::quad()
            .pos(x, y)
            .size(size, size)
            .color(HOVER)
            .border_radius([4.0; 4])
            .build(ui);
    }
    nuon::label()
        .icon(glyph)
        .pos(x, y)
        .size(size, size)
        .font_size(font_size)
        .color(if ev.is_hovered() { TEXT } else { ICON })
        .text_justify(TextAlign::Center)
        .build(ui);
    ev.is_clicked()
}

fn is_tree_key(key: &Key) -> bool {
    matches!(
        key,
        Key::Named(
            NamedKey::ArrowUp
                | NamedKey::ArrowDown
                | NamedKey::ArrowLeft
                | NamedKey::ArrowRight
                | NamedKey::Enter
        )
    )
}

fn is_score(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| !e.eq_ignore_ascii_case("mid") && !e.eq_ignore_ascii_case("midi"))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Cut `text` to about `width` logical pixels at `font_size`, ending with "…"
fn ellipsize(text: &str, width: f32, font_size: f32) -> String {
    let char_w = |c: char| {
        if c.is_ascii() {
            if c.is_ascii_uppercase() || c.is_ascii_digit() {
                0.62
            } else {
                0.52
            }
        } else {
            1.0
        }
    } * font_size;
    let total: f32 = text.chars().map(char_w).sum();
    if total <= width {
        return text.to_string();
    }
    let ellipsis = font_size;
    let mut out = String::new();
    let mut used = 0.0;
    for c in text.chars() {
        let cw = char_w(c);
        if used + cw + ellipsis > width {
            break;
        }
        used += cw;
        out.push(c);
    }
    out.push('\u{2026}');
    out
}

/// Connect the MIDI output and input chosen in the settings
fn connect_io(ctx: &mut Context) {
    let mut state = UiState::new(ctx, None);
    state.tick(ctx);
    menu_scene::connect_io(&state, ctx);
}
