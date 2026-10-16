#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod game;
mod gamepad;
mod host;
mod keys;
mod music;
mod patch;
mod save;
mod settings;
mod settings_window;
mod texts;
mod ui;

use anyhow::{Context, anyhow};
use game::Game;
use host::{AUTOSAVE_SLOT, AppEvent, Commands, Host, SlotNavigator};
use music::Music;
use ruffle_core::backend::navigator::NullExecutor;
use ruffle_core::config::Letterbox;
use ruffle_core::events::{
    KeyDescriptor, KeyLocation, LogicalKey, MouseButton as RuffleMouseButton, MouseWheelDelta,
    NamedKey as RuffleNamedKey, PhysicalKey,
};
use ruffle_core::tag_utils::SwfMovieData;
use ruffle_core::{FloatDuration, Player, PlayerBuilder, PlayerEvent, ViewportDimensions};
use ruffle_frontend_utils::backends::audio::CpalAudioBackend;
use ruffle_render::quality::StageQuality;
use ruffle_render_wgpu::backend::WgpuRenderBackend;
use ruffle_render_wgpu::wgpu;
use settings::{Quality, Settings};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use texts::Texts;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, KeyEvent, Modifiers, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Fullscreen, Window, WindowId};

const BEST_QUALITY: StageQuality = StageQuality::High8x8Linear;
const VOLUME_STEP: u8 = 10;
/// Pointer speed for a fully deflected stick, in logical pixels per second.
const STICK_SPEED: f64 = 700.0;
/// Stage size of the 2001 movie.
const STAGE: (f64, f64) = (800.0, 600.0);
/// The remote icon top right opens the game's menu during play.
const OPEN_MENU: (f64, f64) = (763.0, 39.0);

struct Running {
    window: Arc<Window>,
    player: Arc<Mutex<Player>>,
    executor: NullExecutor,
    last_tick: Instant,
    next_frame: Instant,
    mouse: PhysicalPosition<f64>,
    modifiers: Modifiers,
    occluded: bool,
}

struct App {
    swf: Vec<u8>,
    host: Rc<RefCell<Host>>,
    settings: Settings,
    running: Option<Running>,
    settings_window: Option<settings_window::SettingsWindow>,
    gamepad: gamepad::Gamepad,
    last_poll: Instant,
}

impl App {
    fn start(&mut self, event_loop: &ActiveEventLoop) -> anyhow::Result<Running> {
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title("Zlatý Kalich")
                    .with_inner_size(LogicalSize::new(1200.0, 900.0))
                    .with_min_inner_size(LogicalSize::new(400.0, 300.0)),
            )?,
        );
        let size = window.inner_size();

        // SAFETY: the window is kept alive in `Running` alongside the player
        // that owns the surface.
        let renderer = unsafe {
            WgpuRenderBackend::for_window_unsafe(
                wgpu::SurfaceTargetUnsafe::from_window(&*window)?,
                (size.width.max(1), size.height.max(1)),
                wgpu::Backends::PRIMARY,
                wgpu::PowerPreference::HighPerformance,
                Some(Box::new(window.clone())),
            )
        }
        .map_err(|e| anyhow!("{e}"))?;

        let executor = NullExecutor::new();
        let mut builder = PlayerBuilder::new()
            .with_renderer(renderer)
            .with_navigator(SlotNavigator::new(self.host.clone(), &executor)?)
            .with_fs_commands(Box::new(Commands(self.host.clone())))
            .with_ui(ui::Ui::new(window.clone()))
            .with_letterbox(Letterbox::On)
            .with_autoplay(true);
        if self.settings.quality == Quality::Best {
            builder = builder.with_quality(BEST_QUALITY);
        }
        match CpalAudioBackend::new(None) {
            Ok(audio) => builder = builder.with_audio(audio),
            Err(e) => tracing::error!("No sound effects: {e}"),
        }
        let player = builder.build();

        // Relative URLs in the movie (loadVariables "kalich.sav") resolve
        // against the save directory.
        let save_dir = self.host.borrow().save_dir.clone();
        let url = url::Url::from_file_path(save_dir.join("kalich.swf"))
            .map_err(|()| anyhow!("bad save directory"))?;
        let movie = SwfMovieData::from_data(&self.swf, url.into(), None, None)?;
        {
            let mut player = player.lock().unwrap();
            player.update(|uc| uc.set_root_movie(movie));
            player.set_viewport_dimensions(ViewportDimensions {
                width: size.width,
                height: size.height,
                scale_factor: window.scale_factor(),
            });
        }
        if let Some(on) = self.settings.fullscreen {
            window.set_fullscreen(on.then_some(Fullscreen::Borderless(None)));
        }

        let now = Instant::now();
        let running = Running {
            window,
            player,
            executor,
            last_tick: now,
            next_frame: now,
            mouse: PhysicalPosition::new(0.0, 0.0),
            modifiers: Modifiers::default(),
            occluded: false,
        };
        self.update_title(&running);
        Ok(running)
    }

    fn update_title(&self, running: &Running) {
        let host = self.host.borrow();
        let volume = self.settings.music_volume;
        let title = match (host.english, host.slot) {
            (false, AUTOSAVE_SLOT) => {
                format!("Zlatý Kalich — automatická pozice · hudba {volume} %")
            }
            (false, slot) => format!("Zlatý Kalich — pozice {slot} · hudba {volume} %"),
            (true, AUTOSAVE_SLOT) => format!("Zlatý Kalich — autosave · music {volume}%"),
            (true, slot) => format!("Zlatý Kalich — slot {slot} · music {volume}%"),
        };
        running.window.set_title(&title);
    }

    fn set_fullscreen(&mut self, on: bool) {
        if let Some(r) = &self.running {
            r.window
                .set_fullscreen(on.then_some(Fullscreen::Borderless(None)));
        }
        self.settings.fullscreen = Some(on);
        self.settings.save();
    }

    fn open_settings(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(w) = &self.settings_window {
            return w.focus();
        }
        let english = self.host.borrow().english;
        match settings_window::SettingsWindow::open(event_loop, english) {
            Ok(w) => self.settings_window = Some(w),
            Err(e) => tracing::error!("Couldn't open settings: {e:#}"),
        }
    }

    fn apply(&mut self, changes: settings_window::Changes) {
        if changes.volume
            && let Some(music) = &self.host.borrow().music
        {
            music.set_volume(f32::from(self.settings.music_volume) / 100.0);
        }
        if changes.fullscreen
            && let (Some(on), Some(r)) = (self.settings.fullscreen, &self.running)
        {
            r.window
                .set_fullscreen(on.then_some(Fullscreen::Borderless(None)));
        }
        if changes.slot {
            self.host.borrow_mut().slot = self.settings.slot;
        }
        if changes.quality
            && self.settings.quality == Quality::Game
            && let Some(r) = &self.running
        {
            r.player.lock().unwrap().set_quality(StageQuality::High);
        }
        let any = changes.volume
            || changes.quality
            || changes.fullscreen
            || changes.slot
            || changes.needs_restart;
        if any {
            self.settings.save();
            if let Some(r) = &self.running {
                self.update_title(r);
            }
        }
    }

    /// Maps a point on the 800×600 stage to window pixels (letterboxed).
    fn stage_to_window(&self, (x, y): (f64, f64)) -> Option<PhysicalPosition<f64>> {
        let size = self.running.as_ref()?.window.inner_size();
        let (w, h) = (f64::from(size.width), f64::from(size.height));
        let scale = (w / STAGE.0).min(h / STAGE.1);
        Some(PhysicalPosition::new(
            (w - STAGE.0 * scale) / 2.0 + x * scale,
            (h - STAGE.1 * scale) / 2.0 + y * scale,
        ))
    }

    fn move_pointer(&mut self, to: PhysicalPosition<f64>) {
        let Some(r) = &mut self.running else { return };
        let _ = r.window.set_cursor_position(to);
        r.mouse = to;
        r.send(PlayerEvent::MouseMove { x: to.x, y: to.y });
    }

    fn mouse_button(&mut self, button: RuffleMouseButton, down: bool) {
        let Some(r) = &mut self.running else { return };
        let (x, y) = (r.mouse.x, r.mouse.y);
        r.send(if down {
            PlayerEvent::MouseDown {
                x,
                y,
                button,
                index: None,
            }
        } else {
            PlayerEvent::MouseUp { x, y, button }
        });
    }

    fn poll_gamepad(&mut self, event_loop: &ActiveEventLoop) {
        let dt = self.last_poll.elapsed().as_secs_f64().min(0.1);
        self.last_poll = Instant::now();
        for action in self.gamepad.poll() {
            match action {
                gamepad::PadAction::Click(down) => self.mouse_button(RuffleMouseButton::Left, down),
                gamepad::PadAction::RightClick(down) => {
                    self.mouse_button(RuffleMouseButton::Right, down)
                }
                gamepad::PadAction::Highlight(down) => {
                    let Some(r) = &mut self.running else { continue };
                    let key = KeyDescriptor {
                        physical_key: PhysicalKey::Space,
                        logical_key: LogicalKey::Character(' '),
                        key_location: KeyLocation::Standard,
                    };
                    r.send(if down {
                        PlayerEvent::KeyDown { key }
                    } else {
                        PlayerEvent::KeyUp { key }
                    });
                }
                gamepad::PadAction::GameMenu => {
                    if let Some(at) = self.stage_to_window(OPEN_MENU) {
                        self.move_pointer(at);
                        self.mouse_button(RuffleMouseButton::Left, true);
                        self.mouse_button(RuffleMouseButton::Left, false);
                    }
                }
                gamepad::PadAction::Settings => self.open_settings(event_loop),
            }
        }
        let (sx, sy) = self.gamepad.stick();
        if (sx, sy) != (0.0, 0.0)
            && let Some(r) = &self.running
        {
            let size = r.window.inner_size();
            let step = STICK_SPEED * r.window.scale_factor() * dt;
            let to = PhysicalPosition::new(
                (r.mouse.x + f64::from(sx) * step).clamp(0.0, f64::from(size.width)),
                (r.mouse.y + f64::from(sy) * step).clamp(0.0, f64::from(size.height)),
            );
            self.move_pointer(to);
        }
    }

    /// App shortcuts; returns whether the key was consumed.
    fn shortcut(&mut self, event_loop: &ActiveEventLoop, event: &KeyEvent) -> bool {
        let Some(r) = &self.running else {
            return false;
        };
        if event.state != ElementState::Pressed {
            return false;
        }
        let state = r.modifiers.state();
        let command = state.control_key() || state.super_key();
        let fullscreen = r.window.fullscreen().is_some();
        match event.logical_key.as_ref() {
            Key::Named(NamedKey::Escape) if fullscreen => self.set_fullscreen(false),
            Key::Named(NamedKey::F11) => self.set_fullscreen(!fullscreen),
            Key::Named(NamedKey::Enter) if state.alt_key() => self.set_fullscreen(!fullscreen),
            Key::Named(NamedKey::F1) => self.open_settings(event_loop),
            Key::Character(",") if command => self.open_settings(event_loop),
            Key::Named(NamedKey::ArrowUp) if command => self.change_volume(true),
            Key::Named(NamedKey::ArrowDown) if command => self.change_volume(false),
            Key::Character(c) if command && c.len() == 1 && c.as_bytes()[0].is_ascii_digit() => {
                let slot = c.as_bytes()[0] - b'0';
                self.host.borrow_mut().slot = slot;
                self.settings.slot = slot;
                self.settings.save();
            }
            _ => return false,
        }
        if let Some(r) = &self.running {
            self.update_title(r);
        }
        true
    }

    fn change_volume(&mut self, up: bool) {
        let v = self.settings.music_volume;
        self.settings.music_volume = if up {
            v.saturating_add(VOLUME_STEP).min(100)
        } else {
            v.saturating_sub(VOLUME_STEP)
        };
        if let Some(music) = &self.host.borrow().music {
            music.set_volume(f32::from(self.settings.music_volume) / 100.0);
        }
        self.settings.save();
    }

    /// Asks the game to save itself into the autosave slot.
    fn autosave(&mut self) {
        let Some(r) = &mut self.running else { return };
        let key = KeyDescriptor {
            physical_key: PhysicalKey::F13,
            logical_key: LogicalKey::Named(RuffleNamedKey::F13),
            key_location: KeyLocation::Standard,
        };
        r.send(PlayerEvent::KeyDown { key });
        r.send(PlayerEvent::KeyUp { key });
        self.host.borrow_mut().finish_autosave();
    }
}

impl Running {
    fn send(&mut self, event: PlayerEvent) {
        self.player.lock().unwrap().handle_event(event);
        self.executor.run();
    }

    fn key(&mut self, event: KeyEvent) {
        let key = keys::winit_input_to_ruffle_key_descriptor(&event);
        match event.state {
            ElementState::Pressed => {
                self.send(PlayerEvent::KeyDown { key });
                if let Some(code) = keys::winit_to_ruffle_text_control(&event, self.modifiers) {
                    self.send(PlayerEvent::TextControl { code });
                } else if let Some(text) = event.text {
                    for codepoint in text.chars() {
                        self.send(PlayerEvent::TextInput { codepoint });
                    }
                }
            }
            ElementState::Released => self.send(PlayerEvent::KeyUp { key }),
        }
    }
}

impl ApplicationHandler<AppEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.running.is_some() {
            return;
        }
        match self.start(event_loop) {
            Ok(running) => self.running = Some(running),
            Err(e) => {
                fatal(&format!("{e:#}"));
                event_loop.exit();
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::Quit => {
                self.autosave();
                event_loop.exit();
            }
            // An F11 choice overrides the game's own display option.
            AppEvent::Fullscreen(on) if self.settings.fullscreen.is_none() => {
                if let Some(r) = &self.running {
                    r.window
                        .set_fullscreen(on.then_some(Fullscreen::Borderless(None)));
                }
            }
            AppEvent::Fullscreen(_) => {}
            AppEvent::Title => {
                if let Some(r) = &self.running {
                    self.update_title(r);
                }
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if let Some(w) = &mut self.settings_window
            && w.id() == id
        {
            let english = self.host.borrow().english;
            match w.handle(&event, &mut self.settings, english) {
                Some(changes) => self.apply(changes),
                None => self.settings_window = None,
            }
            return;
        }
        if let WindowEvent::KeyboardInput { event, .. } = &event
            && self.shortcut(event_loop, event)
        {
            return;
        }
        if matches!(event, WindowEvent::CloseRequested) {
            self.autosave();
            event_loop.exit();
            return;
        }
        let Some(r) = &mut self.running else { return };
        match event {
            WindowEvent::Resized(size) => {
                r.player
                    .lock()
                    .unwrap()
                    .set_viewport_dimensions(ViewportDimensions {
                        width: size.width.max(1),
                        height: size.height.max(1),
                        scale_factor: r.window.scale_factor(),
                    });
                r.window.request_redraw();
            }
            WindowEvent::Occluded(occluded) => r.occluded = occluded,
            WindowEvent::RedrawRequested if !r.occluded => {
                r.player.lock().unwrap().render();
            }
            WindowEvent::CursorMoved { position, .. } => {
                r.mouse = position;
                r.send(PlayerEvent::MouseMove {
                    x: position.x,
                    y: position.y,
                });
            }
            WindowEvent::CursorLeft { .. } => r.send(PlayerEvent::MouseLeave),
            WindowEvent::MouseInput { state, button, .. } => {
                let button = match button {
                    MouseButton::Left => RuffleMouseButton::Left,
                    MouseButton::Right => RuffleMouseButton::Right,
                    MouseButton::Middle => RuffleMouseButton::Middle,
                    _ => RuffleMouseButton::Unknown,
                };
                let (x, y) = (r.mouse.x, r.mouse.y);
                r.send(match state {
                    ElementState::Pressed => PlayerEvent::MouseDown {
                        x,
                        y,
                        button,
                        index: None,
                    },
                    ElementState::Released => PlayerEvent::MouseUp { x, y, button },
                });
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let delta = match delta {
                    MouseScrollDelta::LineDelta(_, dy) => MouseWheelDelta::Lines(dy.into()),
                    MouseScrollDelta::PixelDelta(pos) => MouseWheelDelta::Pixels(pos.y),
                };
                r.send(PlayerEvent::MouseWheel { delta });
            }
            WindowEvent::ModifiersChanged(modifiers) => r.modifiers = modifiers,
            WindowEvent::KeyboardInput { event, .. } => r.key(event),
            WindowEvent::Focused(true) => r.send(PlayerEvent::FocusGained),
            WindowEvent::Focused(false) => r.send(PlayerEvent::FocusLost),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.poll_gamepad(event_loop);
        let pad = self.gamepad.connected();
        let Some(r) = &mut self.running else { return };
        let now = Instant::now();
        if now >= r.next_frame {
            let mut player = r.player.lock().unwrap();
            player.tick(FloatDuration::from_std(now - r.last_tick));
            // The game lowers quality from its options menu; keep it crisp.
            if self.settings.quality == Quality::Best && player.quality() != BEST_QUALITY {
                player.set_quality(BEST_QUALITY);
            }
            r.last_tick = now;
            r.next_frame = now + player.time_til_next_frame();
            if player.needs_render() {
                r.window.request_redraw();
            }
        }
        r.executor.run();
        // A connected controller needs polling faster than the 12 fps game.
        let wake = if pad {
            r.next_frame.min(now + std::time::Duration::from_millis(16))
        } else {
            r.next_frame
        };
        event_loop.set_control_flow(ControlFlow::WaitUntil(wake));
    }
}

/// Where an installer given instead of the game gets unpacked.
fn install_dir() -> anyhow::Result<PathBuf> {
    Ok(data_dir()?.join("game"))
}

/// CLI argument (game, its folder, or the original installer), then the
/// remembered location, then the folder next to this
/// binary, then ask.
fn find_game(arg: Option<&Path>, settings: &mut Settings) -> anyhow::Result<Game> {
    if let Some(arg) = arg {
        let game = Game::open_or_install(arg, &install_dir()?)?;
        settings.game_path = Some(game.dir.clone());
        return Ok(game);
    }
    if let Some(saved) = &settings.game_path
        && let Ok(game) = Game::open(saved)
    {
        return Ok(game);
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_owned))
        && let Ok(game) = Game::open(&dir)
    {
        return Ok(game);
    }
    let picked = rfd::FileDialog::new()
        .set_title("Najdi ZlatyKalich.exe nebo instalačku 32-ZKinstall.exe")
        .add_filter("Zlatý Kalich", &["exe", "EXE"])
        .pick_file()
        .context("no game selected")?;
    let game = Game::open_or_install(&picked, &install_dir()?)?;
    settings.game_path = Some(game.dir.clone());
    Ok(game)
}

fn data_dir() -> anyhow::Result<PathBuf> {
    let dir = dirs::data_dir()
        .context("no data directory")?
        .join("zlaty-kalich");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn fatal(message: &str) {
    tracing::error!("{message}");
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("Zlatý Kalich")
        .set_description(message)
        .show();
}

fn run() -> anyhow::Result<()> {
    // Flash 5 movies carry text in the system code page; the game was made
    // on Czech Windows. Ruffle otherwise assumes Windows-1252 (ř -> ø).
    swf::set_legacy_encoding(encoding_rs::WINDOWS_1250);

    let mut args = std::env::args_os().skip(1).map(PathBuf::from);
    let mut settings = Settings::load();
    let first = args.next();
    if first.as_deref() == Some(Path::new("--export-texts")) {
        let out = args.next().context("usage: --export-texts FILE [GAME]")?;
        let game = find_game(args.next().as_deref(), &mut settings)?;
        std::fs::write(&out, texts::export(&game.swf)?)?;
        println!("Wrote {}", out.display());
        return Ok(());
    }
    let game = find_game(first.as_deref(), &mut settings)?;
    settings.save();

    let save_dir = data_dir()?;
    host::migrate_saves(&save_dir, &game.dir)?;
    let user_texts = std::fs::read_to_string(save_dir.join("texts.tsv")).ok();
    let texts = Texts::load(user_texts.as_deref())?;
    let swf = patch::patch(&game.swf, &texts.static_translations(), settings.hotspots)?;

    let music = Music::new(
        settings.soundfont.clone(),
        f32::from(settings.music_volume) / 100.0,
    )
    .inspect_err(|e| tracing::error!("No music: {e:#}"))
    .ok();
    let event_loop = EventLoop::<AppEvent>::with_user_event().build()?;
    let host = Host::new(game.dir.clone(), save_dir, settings.slot, music, texts, {
        let proxy = event_loop.create_proxy();
        move |event| {
            let _ = proxy.send_event(event);
        }
    });
    let mut app = App {
        swf,
        host: Rc::new(RefCell::new(host)),
        settings,
        running: None,
        settings_window: None,
        gamepad: gamepad::Gamepad::new(),
        last_poll: Instant::now(),
    };
    event_loop.run_app(&mut app)?;
    if let Some(music) = &app.host.borrow().music {
        music.stop();
    }
    Ok(())
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "warn,zlaty_kalich=info,symphonia_bundle_mp3=error,ruffle_render_wgpu=error".into()
            }),
        )
        .init();
    if let Err(e) = run() {
        fatal(&format!("{e:#}"));
        std::process::exit(1);
    }
}

#[cfg(test)]
mod headless {
    //! Plays the real game offscreen through the real host (slots, saves,
    //! fscommands) and saves PNGs, without a window:
    //! `ZK_GAME=/path/to/game ZK_SHOTS=/tmp/out cargo test headless -- --ignored`
    use super::*;
    use ruffle_core::events::MouseButton;
    use ruffle_render_wgpu::target::TextureTarget;
    use std::any::Any;

    pub struct Headless {
        pub host: Rc<RefCell<Host>>,
        player: Arc<Mutex<Player>>,
        executor: NullExecutor,
        out: PathBuf,
    }

    impl Headless {
        pub fn new(saves: &Path, hotspots: settings::Hotspots) -> Self {
            swf::set_legacy_encoding(encoding_rs::WINDOWS_1250);
            let game = Game::open(Path::new(&std::env::var("ZK_GAME").unwrap())).unwrap();
            let out = PathBuf::from(std::env::var("ZK_SHOTS").unwrap());
            std::fs::create_dir_all(&out).unwrap();
            host::migrate_saves(saves, &game.dir).unwrap();
            let texts = Texts::load(None).unwrap();
            let swf = patch::patch(&game.swf, &texts.static_translations(), hotspots).unwrap();
            let host = Rc::new(RefCell::new(Host::new(
                game.dir.clone(),
                saves.to_owned(),
                1,
                None,
                texts,
                |_| {},
            )));

            let renderer = WgpuRenderBackend::for_offscreen(
                (800, 600),
                wgpu::Backends::PRIMARY,
                wgpu::PowerPreference::HighPerformance,
            )
            .unwrap();
            let executor = NullExecutor::new();
            let player = PlayerBuilder::new()
                .with_renderer(renderer)
                .with_navigator(SlotNavigator::new(host.clone(), &executor).unwrap())
                .with_fs_commands(Box::new(Commands(host.clone())))
                .with_quality(BEST_QUALITY)
                .with_load_behavior(ruffle_core::LoadBehavior::Blocking)
                .with_autoplay(true)
                .build();
            let url = url::Url::from_file_path(saves.join("kalich.swf")).unwrap();
            let movie = SwfMovieData::from_data(&swf, url.into(), None, None).unwrap();
            {
                let mut p = player.lock().unwrap();
                p.update(|uc| uc.set_root_movie(movie));
                p.set_viewport_dimensions(ViewportDimensions {
                    width: 800,
                    height: 600,
                    scale_factor: 1.0,
                });
            }
            Self {
                host,
                player,
                executor,
                out,
            }
        }

        pub fn run(&mut self, seconds: f64) {
            for _ in 0..(seconds * 12.0) as u32 {
                // `tick` would wait on stream sounds, which need an audio device.
                self.player.lock().unwrap().run_frame();
                self.executor.run();
            }
        }

        fn send(&mut self, event: PlayerEvent) {
            self.player.lock().unwrap().handle_event(event);
            self.executor.run();
            self.run(0.2);
        }

        pub fn hover(&mut self, x: f64, y: f64) {
            self.send(PlayerEvent::MouseMove { x, y });
        }

        pub fn click(&mut self, x: f64, y: f64) {
            self.hover(x, y);
            self.send(PlayerEvent::MouseDown {
                x,
                y,
                button: MouseButton::Left,
                index: None,
            });
            self.send(PlayerEvent::MouseUp {
                x,
                y,
                button: MouseButton::Left,
            });
        }

        pub fn autosave(&mut self) {
            let key = KeyDescriptor {
                physical_key: PhysicalKey::F13,
                logical_key: LogicalKey::Named(RuffleNamedKey::F13),
                key_location: KeyLocation::Standard,
            };
            self.send(PlayerEvent::KeyDown { key });
            self.send(PlayerEvent::KeyUp { key });
            self.host.borrow_mut().finish_autosave();
        }

        pub fn shot(&self, name: &str) {
            let mut p = self.player.lock().unwrap();
            p.render();
            let renderer =
                <dyn Any>::downcast_mut::<WgpuRenderBackend<TextureTarget>>(p.renderer_mut())
                    .unwrap();
            let image = renderer.capture_frame().unwrap();
            image.save(self.out.join(format!("{name}.png"))).unwrap();
        }
    }

    // Stage coordinates of the menus, found from the real game.
    const SKIP: (f64, f64) = (742.0, 57.0);
    const CZECH: (f64, f64) = (223.0, 402.0);
    const ENGLISH: (f64, f64) = (575.0, 375.0);
    const REMOTE_START: (f64, f64) = (412.0, 173.0);
    const REMOTE_SAVE: (f64, f64) = (380.0, 210.0);
    const REMOTE_LOAD: (f64, f64) = (414.0, 210.0);

    fn read(path: PathBuf) -> String {
        String::from_utf8(std::fs::read(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"))).unwrap()
    }

    fn field<'a>(save: &'a str, key: &str) -> &'a str {
        let start = save.find(&format!("&{key}=")).unwrap() + key.len() + 2;
        let rest = &save[start..];
        &rest[..rest.find('&').unwrap_or(rest.len())]
    }

    /// Language screen -> main menu -> new game -> first scene.
    fn new_game(h: &mut Headless, language: (f64, f64)) {
        h.run(3.0);
        h.click(SKIP.0, SKIP.1);
        h.run(5.0);
        h.click(language.0, language.1);
        h.run(5.0);
        h.click(SKIP.0, SKIP.1);
        h.run(10.0);
        h.click(REMOTE_START.0, REMOTE_START.1);
        h.run(20.0);
        h.click(SKIP.0, SKIP.1);
        h.run(20.0);
    }

    #[test]
    #[ignore]
    fn headless_hotspots() {
        let saves = tempfile::tempdir().unwrap();
        let mut h = Headless::new(saves.path(), settings::Hotspots::Always);
        new_game(&mut h, CZECH);
        h.shot("hotspots-scene");
    }

    #[test]
    #[ignore]
    fn headless_save_load_autosave() {
        let saves = tempfile::tempdir().unwrap();
        {
            let mut h = Headless::new(saves.path(), settings::Hotspots::Off);
            new_game(&mut h, ENGLISH);
            assert!(h.host.borrow().english, "game reports English");

            h.autosave();
            let auto = read(saves.path().join("autosave.sav"));
            assert_eq!(field(&auto, "english"), "1");
            let frame: u32 = field(&auto, "cf").parse().unwrap();
            assert!(
                (1951..=1991).contains(&frame),
                "autosave in a scene, got {frame}"
            );

            h.host.borrow_mut().slot = 2;
            h.click(OPEN_MENU.0, OPEN_MENU.1);
            h.run(2.0);
            h.click(REMOTE_SAVE.0, REMOTE_SAVE.1);
            h.run(2.0);
            assert_eq!(
                field(&read(saves.path().join("slot2.sav")), "cf"),
                frame.to_string()
            );
            h.shot("saved");
        }
        {
            // Fresh start in Czech, load the English autosave.
            let mut h = Headless::new(saves.path(), settings::Hotspots::Off);
            h.host.borrow_mut().slot = host::AUTOSAVE_SLOT;
            h.run(3.0);
            h.click(SKIP.0, SKIP.1);
            h.run(5.0);
            h.click(CZECH.0, CZECH.1);
            h.run(5.0);
            h.click(SKIP.0, SKIP.1);
            h.run(10.0);
            assert!(!h.host.borrow().english);
            h.click(REMOTE_LOAD.0, REMOTE_LOAD.1);
            h.run(10.0);
            h.shot("loaded");
            assert!(
                h.host.borrow().english,
                "loading restores the saved language"
            );
        }
    }

    #[test]
    #[ignore]
    fn headless_menu_map() {
        let saves = tempfile::tempdir().unwrap();
        let mut h = Headless::new(saves.path(), settings::Hotspots::Always);
        h.run(3.0);
        h.click(SKIP.0, SKIP.1);
        h.run(5.0);
        h.click(CZECH.0, CZECH.1);
        h.run(5.0);
        h.click(SKIP.0, SKIP.1);
        h.run(10.0);
        for (i, (x, y)) in [
            (380.0, 210.0),
            (414.0, 210.0),
            (380.0, 280.0),
            (414.0, 280.0),
            (412.0, 318.0),
        ]
        .into_iter()
        .enumerate()
        {
            h.hover(x, y);
            h.shot(&format!("menu-hover-{i}"));
        }
    }
}
