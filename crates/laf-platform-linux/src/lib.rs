//! Linux platform backends for Local AI Flow.
//!
//! Runtime environment detection drives every capability here: X11 vs
//! Wayland session, AT-SPI2 availability, which injection tools exist
//! (wtype / ydotool / xdotool), /dev/uinput access, portal support, and
//! evdev readability. `doctor()` reports all of it with exact fixes.

#![cfg(target_os = "linux")]

pub mod atspi_insert;
pub mod doctor;
pub mod hotkeys;
pub mod inserter;
pub mod selection;
pub mod session;
pub mod x11_input;

pub use doctor::doctor;
pub use hotkeys::LinuxHotkeys;
pub use inserter::LinuxInserter;
pub use selection::LinuxSelectionReader;
pub use session::{detect_session, SessionType};
