#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod game;
mod keys;
mod music;
mod save;
mod ui;

use anyhow::{Context, anyhow};
use game::Game;
use music::Music;
use ruffle_core::backend::navigator::{NullExecutor, NullNavigatorBackend};
use ruffle_core::config::Letterbox;
use ruffle_core::events::{MouseButton as RuffleMouseButton, MouseWheelDelta};
use ruffle_core::external::FsCommandProvider;
use ruffle_core::tag_utils::SwfMovieData;
use ruffle_core::{FloatDuration, Player, PlayerBuilder, PlayerEvent, ViewportDimensions};
use ruffle_frontend_utils::backends::audio::CpalAudioBackend;
use ruffle_render_wgpu::backend::WgpuRenderBackend;
use ruffle_render_wgpu::wgpu;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, KeyEvent, Modifiers, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Fullscreen, Window, WindowId};

const SAVE_FILE: &str = "kalich.sav";

enum AppEvent {
    Quit,
    Fullscreen(bool),
}

/// Stands in for the projector's `exec` of `player.exe` and its own commands.
struct Commands {
    game_dir: PathBuf,
    save_dir: PathBuf,
    music: Option<Rc<Music>>,
    events: EventLoopProxy<AppEvent>,
}

impl Commands {
    fn player_exe(&self, args: &str) {
        let params = save::param_str(args);
        let params = &params[1.min(params.len())..];
        match params {
            [] => {
                if let Some(music) = &self.music {
                    music.stop();
                }
            }
            [track] => {
                if let Some(music) = &self.music {
                    let path = game::find_in(&self.game_dir, track);
                    if let Err(e) = music.play(&path) {
                        tracing::error!("Couldn't play {track}: {e:#}");
                    }
                }
            }
            _ => {
                let path = self.save_dir.join(SAVE_FILE);
                if let Err(e) = std::fs::write(&path, save::encode(params)) {
                    tracing::error!("Couldn't save to {}: {e}", path.display());
                }
            }
        }
    }
}

impl FsCommandProvider for Commands {
    fn on_fs_command(&self, command: &str, args: &str) -> bool {
        match command {
            "exec" => self.player_exe(args),
            "quit" => {
                let _ = self.events.send_event(AppEvent::Quit);
            }
            "fullscreen" => {
                let _ = self.events.send_event(AppEvent::Fullscreen(args == "true"));
            }
            // The projector's right-click menu; there is none here.
            "showmenu" => {}
            _ => return false,
        }
        true
    }
}

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
    game: Game,
    save_dir: PathBuf,
    events: EventLoopProxy<AppEvent>,
    music: Option<Rc<Music>>,
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
        let navigator = NullNavigatorBackend::with_base_path(&self.save_dir, &executor)?;

        let mut builder = PlayerBuilder::new()
            .with_renderer(renderer)
            .with_navigator(navigator)
            .with_fs_commands(Box::new(Commands {
                game_dir: self.game.dir.clone(),
                save_dir: self.save_dir.clone(),
                music: self.music.clone(),
                events: self.events.clone(),
            }))
            .with_ui(ui::Ui::new(window.clone()))
            .with_letterbox(Letterbox::On)
            .with_autoplay(true);
        match CpalAudioBackend::new(None) {
            Ok(audio) => builder = builder.with_audio(audio),
            Err(e) => tracing::error!("No sound effects: {e}"),
        }
        let player = builder.build();

        // Relative URLs in the movie (loadVariables "kalich.sav") resolve
        // against the save directory.
        let url = url::Url::from_file_path(self.save_dir.join("kalich.swf"))
            .map_err(|()| anyhow!("bad save directory"))?;
        let movie = SwfMovieData::from_data(&self.game.swf, url.into(), None, None)?;
        {
            let mut player = player.lock().unwrap();
            player.update(|uc| uc.set_root_movie(movie));
            player.set_viewport_dimensions(ViewportDimensions {
                width: size.width,
                height: size.height,
                scale_factor: window.scale_factor(),
            });
        }

        let now = Instant::now();
        Ok(Running {
            window,
            player,
            executor,
            last_tick: now,
            next_frame: now,
            mouse: PhysicalPosition::new(0.0, 0.0),
            modifiers: Modifiers::default(),
            occluded: false,
        })
    }
}

impl Running {
    fn send(&mut self, event: PlayerEvent) {
        self.player.lock().unwrap().handle_event(event);
        self.executor.run();
    }

    fn set_fullscreen(&self, on: bool) {
        self.window
            .set_fullscreen(on.then_some(Fullscreen::Borderless(None)));
    }

    fn key(&mut self, event: KeyEvent) {
        if event.state == ElementState::Pressed {
            match event.logical_key {
                Key::Named(NamedKey::Escape) if self.window.fullscreen().is_some() => {
                    return self.set_fullscreen(false);
                }
                Key::Named(NamedKey::F11) => {
                    return self.set_fullscreen(self.window.fullscreen().is_none());
                }
                Key::Named(NamedKey::Enter) if self.modifiers.state().alt_key() => {
                    return self.set_fullscreen(self.window.fullscreen().is_none());
                }
                _ => {}
            }
        }
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
            AppEvent::Quit => event_loop.exit(),
            AppEvent::Fullscreen(on) => {
                if let Some(running) = &self.running {
                    running.set_fullscreen(on);
                }
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(r) = &mut self.running else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
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

fn config_file() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("zlaty-kalich").join("game-path.txt"))
}

/// CLI argument, then the remembered location, then the folder next to this
/// binary, then ask.
fn find_game() -> anyhow::Result<Game> {
    if let Some(arg) = std::env::args_os().nth(1) {
        let game = Game::open(Path::new(&arg))?;
        remember(&game.dir);
        return Ok(game);
    }
    if let Some(saved) = config_file().and_then(|f| std::fs::read_to_string(f).ok())
        && let Ok(game) = Game::open(Path::new(saved.trim()))
    {
        return Ok(game);
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_owned))
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
    remember(&game.dir);
    Ok(game)
}

fn remember(dir: &Path) {
    if let Some(file) = config_file() {
        let _ = std::fs::create_dir_all(file.parent().unwrap());
        let _ = std::fs::write(file, dir.to_string_lossy().as_bytes());
    }
}

fn save_dir(game: &Game) -> anyhow::Result<PathBuf> {
    let dir = dirs::data_dir()
        .context("no data directory")?
        .join("zlaty-kalich");
    std::fs::create_dir_all(&dir)?;
    // Carry over progress from the original install on first run.
    let save = dir.join(SAVE_FILE);
    let original = game.file(SAVE_FILE);
    if !save.exists() && original.exists() {
        std::fs::copy(original, save)?;
    }
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
    ruffle_core::swf::set_legacy_encoding(encoding_rs::WINDOWS_1250);
    let game = find_game()?;
    let save_dir = save_dir(&game)?;
    let music = match Music::new() {
        Ok(music) => Some(Rc::new(music)),
        Err(e) => {
            tracing::error!("No music: {e:#}");
            None
        }
    };
    let event_loop = EventLoop::<AppEvent>::with_user_event().build()?;
    let mut app = App {
        game,
        save_dir,
        events: event_loop.create_proxy(),
        music,
        running: None,
    };
    event_loop.run_app(&mut app)?;
    if let Some(music) = &app.music {
        music.stop();
    }
    Ok(())
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,zlaty_kalich=info,symphonia_bundle_mp3=error".into()),
        )
        .init();
    if let Err(e) = run() {
        fatal(&format!("{e:#}"));
        std::process::exit(1);
    }
}
