//! Local AI Flow — shared, platform-agnostic core.
//!
//! Everything OS-specific (audio devices, key injection, accessibility APIs,
//! hotkeys, permissions) lives behind the traits in [`traits`]; the two
//! platform crates (`laf-platform-macos`, `laf-platform-linux`) and the
//! engine crate (`laf-engines`) provide the implementations. This crate owns:
//!
//! * the dictation [`pipeline`] state machine,
//! * the deterministic [`clean`] tier (fillers, punctuation, spoken commands),
//! * [`modes`] (Raw / Auto / Email / Message / List / Code / Command),
//! * the custom [`dictionary`],
//! * [`settings`] persistence,
//! * the [`models`] manager (the only place in the app allowed to touch the
//!   network, and only when the user explicitly asks for a download),
//! * [`doctor`] report types and local-only [`metrics`].

pub mod clean;
pub mod dictionary;
pub mod doctor;
pub mod hotkeys;
pub mod metrics;
pub mod models;
pub mod modes;
pub mod pipeline;
pub mod resample;
pub mod settings;
pub mod traits;
pub mod types;
pub mod vad;

pub use types::*;
