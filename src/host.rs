//! The app side of the host API: everything the game asks of the outside
//! world, plus save slots.

use crate::music::Music;
use crate::patch;
use crate::save;
use crate::texts::Texts;
use ruffle_core::backend::navigator::{
    ErrorResponse, NavigationMethod, NavigatorBackend, NullExecutor, NullNavigatorBackend,
    OwnedFuture, Request, SuccessResponse,
};
use ruffle_core::external::FsCommandProvider;
use ruffle_core::indexmap::IndexMap;
use ruffle_core::loader::Error;
use ruffle_core::socket::{ConnectionState, SocketAction, SocketHandle};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;
use url::{ParseError, Url};
use winit::event_loop::EventLoopProxy;

/// File name the movie saves to and loads from.
pub const SAVE_FILE: &str = "kalich.sav";
pub const AUTOSAVE_SLOT: u8 = 0;

pub enum AppEvent {
    Quit,
    Fullscreen(bool),
    /// Language or slot changed; refresh the window title.
    Title,
}

pub struct Host {
    pub game_dir: PathBuf,
    pub save_dir: PathBuf,
    pub slot: u8,
    pub english: bool,
    pub music: Option<Music>,
    pub texts: Texts,
    /// Set by the host clip just before an autosave's `savegame()`.
    autosave_next: bool,
    events: EventLoopProxy<AppEvent>,
}

impl Host {
    pub fn new(
        game_dir: PathBuf,
        save_dir: PathBuf,
        slot: u8,
        music: Option<Music>,
        texts: Texts,
        events: EventLoopProxy<AppEvent>,
    ) -> Self {
        swf::set_text_overrides(texts.overrides(false));
        Self {
            game_dir,
            save_dir,
            slot,
            english: false,
            music,
            texts,
            autosave_next: false,
            events,
        }
    }

    pub fn slot_path(&self, slot: u8) -> PathBuf {
        self.save_dir.join(if slot == AUTOSAVE_SLOT {
            "autosave.sav".to_owned()
        } else {
            format!("slot{slot}.sav")
        })
    }

    /// Clears a pending autosave the game didn't act on (no game running).
    pub fn finish_autosave(&mut self) {
        self.autosave_next = false;
    }

    fn player_exe(&mut self, args: &str) {
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
                    let path = crate::game::find_in(&self.game_dir, track);
                    if let Err(e) = music.play(&path) {
                        tracing::error!("Couldn't play {track}: {e:#}");
                    }
                }
            }
            _ => {
                let slot = if std::mem::take(&mut self.autosave_next) {
                    AUTOSAVE_SLOT
                } else {
                    self.slot
                };
                let path = self.slot_path(slot);
                match std::fs::write(&path, save::encode(params)) {
                    Ok(()) => tracing::info!("Saved to {}", path.display()),
                    Err(e) => tracing::error!("Couldn't save to {}: {e}", path.display()),
                }
            }
        }
    }

    fn host_command(&mut self, args: &str) {
        if args == "autosave" {
            self.autosave_next = true;
        } else if let Some(value) = args.strip_prefix("english=") {
            self.english = value == "1";
            swf::set_text_overrides(self.texts.overrides(self.english));
            let _ = self.events.send_event(AppEvent::Title);
        }
    }
}

/// Moves pre-slot saves into slot 1, and carries over the save from the
/// original install on first run.
pub fn migrate_saves(save_dir: &Path, game_dir: &Path) -> std::io::Result<()> {
    let slot1 = save_dir.join("slot1.sav");
    if slot1.exists() {
        return Ok(());
    }
    let legacy = save_dir.join(SAVE_FILE);
    if legacy.exists() {
        return std::fs::rename(legacy, slot1);
    }
    let original = crate::game::find_in(game_dir, SAVE_FILE);
    if original.exists() {
        std::fs::copy(original, slot1)?;
    }
    Ok(())
}

pub struct Commands(pub Rc<RefCell<Host>>);

impl FsCommandProvider for Commands {
    fn on_fs_command(&self, command: &str, args: &str) -> bool {
        let mut host = self.0.borrow_mut();
        match command {
            "exec" => host.player_exe(args),
            patch::host::COMMAND => host.host_command(args),
            "quit" => {
                let _ = host.events.send_event(AppEvent::Quit);
            }
            "fullscreen" => {
                let _ = host.events.send_event(AppEvent::Fullscreen(args == "true"));
            }
            // The projector's right-click menu; there is none here.
            "showmenu" => {}
            _ => return false,
        }
        true
    }
}

/// Serves `loadVariables("kalich.sav")` from the selected slot.
pub struct SlotNavigator {
    inner: NullNavigatorBackend,
    host: Rc<RefCell<Host>>,
}

impl SlotNavigator {
    pub fn new(host: Rc<RefCell<Host>>, executor: &NullExecutor) -> std::io::Result<Self> {
        let inner = NullNavigatorBackend::with_base_path(&host.borrow().save_dir, executor)?;
        Ok(Self { inner, host })
    }
}

impl NavigatorBackend for SlotNavigator {
    fn navigate_to_url(
        &self,
        url: &str,
        target: &str,
        vars_method: Option<(NavigationMethod, IndexMap<String, String>)>,
    ) {
        self.inner.navigate_to_url(url, target, vars_method);
    }

    fn fetch(&self, mut request: Request) -> OwnedFuture<Box<dyn SuccessResponse>, ErrorResponse> {
        if request.url().ends_with(SAVE_FILE) {
            let host = self.host.borrow();
            if let Ok(url) = Url::from_file_path(host.slot_path(host.slot)) {
                request.set_url(url.into());
            }
        }
        self.inner.fetch(request)
    }

    fn resolve_url(&self, url: &str) -> Result<Url, ParseError> {
        self.inner.resolve_url(url)
    }

    fn spawn_future(&mut self, future: OwnedFuture<(), Error>) {
        self.inner.spawn_future(future);
    }

    fn pre_process_url(&self, url: Url) -> Url {
        self.inner.pre_process_url(url)
    }

    fn connect_socket(
        &mut self,
        _host: String,
        _port: u16,
        _timeout: Duration,
        handle: SocketHandle,
        _receiver: async_channel::Receiver<Vec<u8>>,
        sender: async_channel::Sender<SocketAction>,
    ) {
        let _ = sender.try_send(SocketAction::Connect(handle, ConnectionState::Failed));
    }
}
