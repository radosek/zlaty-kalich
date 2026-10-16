//! Platform-independent parts of the Zlatý Kalich port: reading the game,
//! patching the movie, editable text and the save format. Shared by the
//! desktop app and the browser build.

pub mod bytecode;
pub mod patch;
pub mod projector;
pub mod save;
pub mod texts;

pub use projector::extract_swf;
