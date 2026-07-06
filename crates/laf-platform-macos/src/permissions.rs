//! macOS permission checks + onboarding helpers.
//!
//! * **Accessibility** — required for AX insertion, synthetic events, and
//!   selection reading. `AXIsProcessTrustedWithOptions` can show the system
//!   prompt that deep-links to the settings pane.
//! * **Input Monitoring** — NOT required by the current design: hotkeys use
//!   Carbon `RegisterEventHotKey` (via the global-hotkey crate) and we never
//!   install event taps. We still expose its status because users coming
//!   from other dictation apps expect to see it.
//! * **Microphone** — requested automatically by macOS on first capture
//!   (`NSMicrophoneUsageDescription` is set in Info.plist).

use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
}

pub fn accessibility_trusted() -> bool {
    unsafe { crate::ax::AXIsProcessTrusted() != 0 }
}

/// Check and (optionally) show the system Accessibility prompt.
pub fn request_accessibility(prompt: bool) -> bool {
    unsafe {
        let key = CFString::wrap_under_get_rule(crate::ax::kAXTrustedCheckOptionPrompt);
        let value = CFBoolean::from(prompt);
        let options =
            CFDictionary::from_CFType_pairs(&[(key.as_CFType(), value.as_CFType())]);
        crate::ax::AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) != 0
    }
}

pub fn input_monitoring_granted() -> bool {
    unsafe { CGPreflightListenEventAccess() }
}

pub fn request_input_monitoring() -> bool {
    unsafe { CGRequestListenEventAccess() }
}

/// Deep-link into the relevant System Settings pane.
pub fn open_settings_pane(pane: Pane) {
    let url = match pane {
        Pane::Accessibility => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        }
        Pane::Microphone => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
        }
        Pane::InputMonitoring => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
        }
    };
    let _ = std::process::Command::new("/usr/bin/open").arg(url).spawn();
}

#[derive(Debug, Clone, Copy)]
pub enum Pane {
    Accessibility,
    Microphone,
    InputMonitoring,
}
