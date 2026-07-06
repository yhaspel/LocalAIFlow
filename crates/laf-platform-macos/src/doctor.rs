//! macOS environment doctor.

use crate::permissions;
use laf_core::doctor::{DoctorCheck, DoctorReport};

pub fn doctor() -> DoctorReport {
    let mut checks = Vec::new();

    checks.push(if permissions::accessibility_trusted() {
        DoctorCheck::ok(
            "mac.accessibility",
            "Accessibility permission",
            "granted — AX insertion, synthetic events, and selection reading available",
        )
    } else {
        DoctorCheck::fail(
            "mac.accessibility",
            "Accessibility permission",
            "not granted — text insertion and selection reading will fail",
            "System Settings → Privacy & Security → Accessibility → enable Local AI Flow (onboarding has a button)",
        )
    });

    checks.push(if permissions::input_monitoring_granted() {
        DoctorCheck::ok("mac.input_monitoring", "Input Monitoring", "granted (not required by current hotkey design; informational)")
    } else {
        DoctorCheck::warn(
            "mac.input_monitoring",
            "Input Monitoring",
            "not granted — fine: hotkeys use Carbon RegisterEventHotKey which doesn't need it",
            "only needed if a future event-tap feature is enabled",
        )
    });

    checks.push(DoctorCheck::ok(
        "mac.microphone",
        "Microphone",
        "macOS prompts on first dictation (usage description is declared); manage later in System Settings → Privacy & Security → Microphone",
    ));

    // espeak-ng is Kokoro's G2P dependency on macOS too (Homebrew).
    let espeak = ["/opt/homebrew/lib/libespeak-ng.dylib", "/usr/local/lib/libespeak-ng.dylib"]
        .iter()
        .any(|p| std::path::Path::new(p).exists());
    checks.push(if espeak {
        DoctorCheck::ok("mac.espeak", "espeak-ng (Kokoro G2P)", "found")
    } else {
        DoctorCheck::warn(
            "mac.espeak",
            "espeak-ng (Kokoro G2P)",
            "libespeak-ng.dylib not found — the Kokoro neural voice will be unavailable (the built-in `say` fallback still works)",
            "brew install espeak-ng",
        )
    });

    let models = laf_core::settings::data_dir().join("models");
    let writable = std::fs::create_dir_all(&models).is_ok();
    checks.push(if writable {
        DoctorCheck::ok("models.dir", "Models directory", models.display().to_string())
    } else {
        DoctorCheck::fail(
            "models.dir",
            "Models directory",
            format!("cannot create {}", models.display()),
            "check permissions on ~/Library/Application Support",
        )
    });

    let os = std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());

    DoctorReport { platform: "macos".into(), session: format!("macOS {os}"), checks }
}
