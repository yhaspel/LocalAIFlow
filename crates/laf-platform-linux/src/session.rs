//! Session-type detection and external-tool discovery.

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionType {
    Wayland,
    X11,
    Unknown,
}

impl SessionType {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionType::Wayland => "wayland",
            SessionType::X11 => "x11",
            SessionType::Unknown => "unknown",
        }
    }
}

/// Detect the active session type at runtime.
/// Precedence: XDG_SESSION_TYPE, then WAYLAND_DISPLAY, then DISPLAY.
pub fn detect_session() -> SessionType {
    match std::env::var("XDG_SESSION_TYPE").as_deref() {
        Ok("wayland") => return SessionType::Wayland,
        Ok("x11") => return SessionType::X11,
        _ => {}
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return SessionType::Wayland;
    }
    if std::env::var_os("DISPLAY").is_some() {
        return SessionType::X11;
    }
    SessionType::Unknown
}

/// Find an executable on PATH.
pub fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(bin)).find(|p| p.is_file())
}

/// Is the ydotool daemon reachable? ydotool(1) talks to ydotoold over a
/// socket; without the daemon the client hangs/fails.
pub fn ydotoold_socket() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(explicit) = std::env::var("YDOTOOL_SOCKET") {
        candidates.push(PathBuf::from(explicit));
    }
    if let Ok(uid) = std::env::var("UID") {
        candidates.push(PathBuf::from(format!("/run/user/{uid}/.ydotool_socket")));
    }
    // getuid without a libc call on the env var being absent:
    let uid = unsafe { libc::getuid() };
    candidates.push(PathBuf::from(format!("/run/user/{uid}/.ydotool_socket")));
    candidates.push(PathBuf::from("/tmp/.ydotool_socket"));
    candidates.into_iter().find(|p| p.exists())
}

/// Can we open /dev/uinput for writing (needed by ydotool's daemon when the
/// user runs it themselves, and informative for our doctor)?
pub fn uinput_writable() -> bool {
    std::fs::OpenOptions::new().write(true).open("/dev/uinput").is_ok()
}

/// Can we read at least one keyboard-capable /dev/input/event* device
/// (required for the evdev hotkey fallback on Wayland)?
pub fn evdev_readable() -> bool {
    let Ok(dir) = std::fs::read_dir("/dev/input") else { return false };
    dir.filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("event")))
        .any(|p| std::fs::OpenOptions::new().read(true).open(p).is_ok())
}

/// Is the AT-SPI2 accessibility bus reachable? (org.a11y.Bus on the session
/// bus answers GetAddress when at-spi2 is running.)
pub async fn atspi_bus_address() -> Option<String> {
    let conn = zbus::Connection::session().await.ok()?;
    let reply = conn
        .call_method(Some("org.a11y.Bus"), "/org/a11y/bus", Some("org.a11y.Bus"), "GetAddress", &())
        .await
        .ok()?;
    reply.body().deserialize::<String>().ok()
}

/// Is the GlobalShortcuts portal present? Checks the interface's `version`
/// property on the desktop portal service.
pub async fn portal_global_shortcuts_version() -> Option<u32> {
    let conn = zbus::Connection::session().await.ok()?;
    let proxy = zbus::fdo::PropertiesProxy::builder(&conn)
        .destination("org.freedesktop.portal.Desktop")
        .ok()?
        .path("/org/freedesktop/portal/desktop")
        .ok()?
        .build()
        .await
        .ok()?;
    let iface =
        zbus::names::InterfaceName::try_from("org.freedesktop.portal.GlobalShortcuts").ok()?;
    let v = proxy.get(iface, "version").await.ok()?;
    u32::try_from(&v).ok()
}
