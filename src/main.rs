#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod game;
mod host;
mod keys;
mod music;
mod patch;
mod save;
mod settings;
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

    /// App shortcuts; returns whether the key was consumed.
    fn shortcut(&mut self, event: &KeyEvent) -> bool {
        let Some(r) = &self.running else {
            return false;
        };
        if event.state != ElementState::Pressed {
            return false;
        }
        let state = r.modifiers.state();
        let command = state.control_key() || state.super_key();
        let fullscreen = r.window.fullscreen().is_some();
        match &event.logical_key {
            Key::Named(NamedKey::Escape) if fullscreen => self.set_fullscreen(false),
            Key::Named(NamedKey::F11) => self.set_fullscreen(!fullscreen),
            Key::Named(NamedKey::Enter) if state.alt_key() => self.set_fullscreen(!fullscreen),
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

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        if let WindowEvent::KeyboardInput { event, .. } = &event
            && self.shortcut(event)
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
        event_loop.set_control_flow(ControlFlow::WaitUntil(r.next_frame));
    }
}

/// CLI argument, then the remembered location, then the folder next to this
/// binary, then ask.
fn find_game(arg: Option<&Path>, settings: &mut Settings) -> anyhow::Result<Game> {
    if let Some(arg) = arg {
        let game = Game::open(arg)?;
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
        .set_title("Najdi ZlatyKalich.exe")
        .add_filter("Zlatý Kalich", &["exe", "EXE"])
        .pick_file()
        .context("no game selected")?;
    let game = Game::open(&picked)?;
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
    let host = Host::new(
        game.dir.clone(),
        save_dir,
        settings.slot,
        music,
        texts,
        event_loop.create_proxy(),
    );
    let mut app = App {
        swf,
        host: Rc::new(RefCell::new(host)),
        settings,
        running: None,
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
