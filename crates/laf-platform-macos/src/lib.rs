//! macOS platform backends for Local AI Flow.
//!
//! * Text insertion chain: AX value/selected-text → CGEvent unicode typing →
//!   NSPasteboard + synthetic ⌘V with clipboard restore.
//! * Selection reading for TTS: AX selected text → clipboard ⌘C fallback.
//! * Permissions: Accessibility (AXIsProcessTrusted…), Input Monitoring
//!   (CGPreflightListenEventAccess), microphone (delegated to the OS prompt
//!   triggered by first capture + Info.plist usage string).

#![cfg(target_os = "macos")]

pub mod ax;
pub mod doctor;
pub mod hotkeys;
pub mod inserter;
pub mod permissions;
pub mod selection;

pub use doctor::doctor;
pub use hotkeys::MacHotkeys;
pub use inserter::MacInserter;
pub use selection::MacSelectionReader;
