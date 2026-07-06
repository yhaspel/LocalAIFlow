//! Linux environment doctor: detects the session type and every capability
//! the insertion/hotkey/TTS chains rely on, with copy-pasteable fixes.

use crate::session::{
    atspi_bus_address, detect_session, evdev_readable, portal_global_shortcuts_version,
    uinput_writable, which, ydotoold_socket, SessionType,
};
use laf_core::doctor::{DoctorCheck, DoctorReport};

pub fn doctor() -> DoctorReport {
    let session = detect_session();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().ok();
    let (atspi_ok, portal_version) = match &rt {
        Some(rt) => rt.block_on(async {
            (atspi_bus_address().await.is_some(), portal_global_shortcuts_version().await)
        }),
        None => (false, None),
    };

    let mut checks: Vec<DoctorCheck> = Vec::new();

    // ---- audio ----
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/run/user/self".into());
    let pipewire = std::path::Path::new(&runtime_dir).join("pipewire-0").exists();
    let pulse = std::path::Path::new(&runtime_dir).join("pulse/native").exists();
    checks.push(if pipewire || pulse {
        DoctorCheck::ok(
            "audio.server",
            "Audio server",
            if pipewire { "PipeWire detected" } else { "PulseAudio detected" },
        )
    } else {
        DoctorCheck::fail(
            "audio.server",
            "Audio server",
            "neither PipeWire nor PulseAudio socket found",
            "install/start pipewire (e.g. `systemctl --user start pipewire pipewire-pulse`)",
        )
    });

    // ---- accessibility bus (insertion rung 1) ----
    checks.push(if atspi_ok {
        DoctorCheck::ok("a11y.bus", "AT-SPI2 accessibility bus", "reachable — direct text insertion available where apps support it")
    } else {
        DoctorCheck::warn(
            "a11y.bus",
            "AT-SPI2 accessibility bus",
            "not reachable; insertion will use synthetic typing / clipboard instead",
            "install at-spi2-core and enable accessibility: `gsettings set org.gnome.desktop.interface toolkit-accessibility true`",
        )
    });

    // ---- injection (insertion rungs 2–3) ----
    match session {
        SessionType::Wayland => {
            let wtype = which("wtype").is_some();
            let ydotool = which("ydotool").is_some();
            let ydotoold = ydotoold_socket().is_some();
            let uinput = uinput_writable();
            checks.push(if wtype {
                DoctorCheck::ok("wayland.wtype", "wtype (virtual-keyboard protocol)", "installed — used on wlroots compositors (Sway/Hyprland)")
            } else {
                DoctorCheck::warn(
                    "wayland.wtype",
                    "wtype (virtual-keyboard protocol)",
                    "not installed (GNOME/KDE don't support the protocol anyway; ydotool covers them)",
                    "optional: install wtype (best path on Sway/Hyprland)",
                )
            });
            checks.push(match (ydotool, ydotoold, uinput) {
                (true, true, _) => DoctorCheck::ok(
                    "wayland.ydotool",
                    "ydotool (kernel uinput injection)",
                    "installed and ydotoold is running — compositor-independent typing available",
                ),
                (true, false, _) => DoctorCheck::warn(
                    "wayland.ydotool",
                    "ydotool (kernel uinput injection)",
                    "installed but ydotoold is not running",
                    "enable the daemon: `systemctl --user enable --now ydotool` (and ensure /dev/uinput access via the udev rule in packaging/99-localaiflow-uinput.rules)",
                ),
                (false, _, _) => DoctorCheck::warn(
                    "wayland.ydotool",
                    "ydotool (kernel uinput injection)",
                    "not installed — on GNOME/KDE Wayland, typing falls back to clipboard paste only",
                    "install ydotool and enable ydotoold (`systemctl --user enable --now ydotool`)",
                ),
            });
            if !uinput {
                checks.push(DoctorCheck::warn(
                    "wayland.uinput",
                    "/dev/uinput access",
                    "not writable by this user (ydotoold may still work if it runs as a system service)",
                    "install packaging/99-localaiflow-uinput.rules to /etc/udev/rules.d/, add user to 'input' group, then `sudo udevadm control --reload && sudo udevadm trigger`",
                ));
            }
        }
        SessionType::X11 | SessionType::Unknown => {
            checks.push(if which("xdotool").is_some() {
                DoctorCheck::ok("x11.xdotool", "xdotool (XTEST typing)", "installed")
            } else {
                DoctorCheck::warn(
                    "x11.xdotool",
                    "xdotool (XTEST typing)",
                    "not installed — unicode typing rung skipped (clipboard paste still works natively)",
                    "install xdotool",
                )
            });
        }
    }

    // ---- hotkeys ----
    match session {
        SessionType::X11 | SessionType::Unknown => checks.push(DoctorCheck::ok(
            "hotkeys.x11",
            "Global hotkeys",
            "X11 key grabs (press & release — push-to-talk supported)",
        )),
        SessionType::Wayland => {
            checks.push(match portal_version {
                Some(v) => DoctorCheck::ok(
                    "hotkeys.portal",
                    "GlobalShortcuts portal",
                    format!("available (version {v}) — press & release supported"),
                ),
                None => {
                    if evdev_readable() {
                        DoctorCheck::warn(
                            "hotkeys.portal",
                            "GlobalShortcuts portal",
                            "portal missing; using raw evdev fallback (works, bypasses compositor)",
                            "update xdg-desktop-portal (KDE: xdg-desktop-portal-kde; GNOME ≥ 48)",
                        )
                    } else {
                        DoctorCheck::fail(
                            "hotkeys.portal",
                            "Global hotkeys on Wayland",
                            "no GlobalShortcuts portal AND /dev/input not readable",
                            "either update xdg-desktop-portal, or `sudo usermod -aG input $USER` and re-login for the evdev fallback",
                        )
                    }
                }
            });
        }
    }

    // ---- TTS ----
    let espeak_lib = ["/usr/lib/aarch64-linux-gnu", "/usr/lib/x86_64-linux-gnu", "/usr/lib", "/usr/lib64", "/usr/local/lib"]
        .iter()
        .any(|d| std::path::Path::new(d).join("libespeak-ng.so.1").exists());
    checks.push(if espeak_lib || which("espeak-ng").is_some() {
        DoctorCheck::ok("tts.espeak", "espeak-ng (Kokoro G2P)", "present")
    } else {
        DoctorCheck::warn(
            "tts.espeak",
            "espeak-ng (Kokoro G2P)",
            "libespeak-ng not found — the Kokoro neural voice cannot phonemize",
            "install espeak-ng (e.g. `sudo apt install espeak-ng libespeak-ng1`)",
        )
    });
    checks.push(if which("spd-say").is_some() || which("espeak-ng").is_some() {
        DoctorCheck::ok("tts.fallback", "System TTS fallback", "speech-dispatcher / espeak-ng available")
    } else {
        DoctorCheck::warn(
            "tts.fallback",
            "System TTS fallback",
            "no spd-say or espeak-ng binary",
            "install speech-dispatcher or espeak-ng for a no-model fallback voice",
        )
    });

    // ---- models dir ----
    let models = laf_core::settings::data_dir().join("models");
    let writable = std::fs::create_dir_all(&models).is_ok();
    checks.push(if writable {
        DoctorCheck::ok("models.dir", "Models directory", models.display().to_string())
    } else {
        DoctorCheck::fail(
            "models.dir",
            "Models directory",
            format!("cannot create {}", models.display()),
            "check permissions on ~/.local/share",
        )
    });

    DoctorReport { platform: "linux".into(), session: session.as_str().into(), checks }
}
