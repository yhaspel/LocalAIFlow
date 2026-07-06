# Manual QA checklist

Run per release on: **macOS** (Apple Silicon + Intel if possible),
**GNOME Wayland**, **KDE Wayland**, and a **generic X11/Xorg** session.
`local-ai-flow --doctor` first on each — it must accurately describe the
environment before anything else is tested.

Legend: ☐ pass required · (opt) = only when the optional dependency is set up.

## 0. Install & first run
- ☐ App starts as tray/menu-bar agent; no Dock icon (macOS) / no taskbar entry
- ☐ First run opens Settings with onboarding banner
- ☐ macOS: Accessibility "Grant…" deep-links to the right pane; status flips
  to ok after granting (live re-check)
- ☐ macOS: mic prompt appears on FIRST dictation, not at launch
- ☐ "Download recommended models" fetches all five with progress bars;
  re-clicking a finished row shows Verify ✓
- ☐ Fully Offline toggle blocks downloads with a clear error
- ☐ `--doctor` exit code: 0 ok / 1 warnings / 2 failures

## 1. Dictation core (each desktop)
- ☐ Toggle hotkey: HUD appears bottom-center, waveform moves with voice
- ☐ Partial transcript streams (< ~300 ms after speech starts, warm model)
- ☐ Stop: clean text inserted into focused field < 1 s (sentence, warm)
- ☐ Push-to-talk: hold → dictate → release inserts; PTT release does NOT stop
  a toggle-started session
- ☐ Toggle press while listening stops it (no double-start)
- ☐ Cancel (tray or settings) inserts nothing
- ☐ Latency Debug tab populates: stt_finalize / clean / insert / e2e

## 2. Insertion matrix (dictate "hello world period test")
Targets: native editor (TextEdit / gedit / kwrite), browser URL bar +
textarea (Firefox & Chromium), terminal, Electron app (e.g. VS Code).

macOS:
- ☐ TextEdit → method `ax_direct` (see Debug event log)
- ☐ Chromium textarea → `ax_direct` or `synthetic_keys`
- ☐ Terminal.app → `synthetic_keys` or `clipboard_paste`; clipboard restored
- ☐ Unicode: dictate an emoji-ish word / non-ASCII name → correct via CGEvent path

Linux X11:
- ☐ gedit → `atspi_editable_text`
- ☐ Firefox textarea → `synthetic_keys` (xdotool) or clipboard fallback
- ☐ xterm → clipboard path types via XTEST Ctrl+V; PRIMARY untouched

GNOME Wayland:
- ☐ gedit → `atspi_editable_text`
- ☐ Firefox → ydotool typing (opt) else `clipboard_paste`; doctor explains missing pieces
- ☐ clipboard restored ~0.7 s after paste

KDE Wayland:
- ☐ kwrite → `atspi_editable_text` or ydotool
- ☐ Konsole → clipboard path (note: terminals wanting Ctrl+Shift+V are documented)

Sway/Hyprland (opt):
- ☐ typing via wtype rung

## 3. Modes & cleanup
- ☐ Auto: "um so i think we should uh ship it" → "So I think we should ship it."
- ☐ Raw: verbatim incl. fillers
- ☐ List: three spoken sentences → three "- " bullets
- ☐ Email: greeting on own line, blank line after
- ☐ Code mode: "snake case my_variable equals five" keeps identifier casing
- ☐ Command mode: "hello period new line delete that" behaves per spec
- ☐ Dictionary: add "cuber netties → Kubernetes", dictate it, substituted (LLM and deterministic tiers)
- ☐ LLM tier active when model installed (Debug shows clean > 0 ms with llama);
  yanking the model file falls back silently to deterministic
- ☐ Ollama tier (opt): local server used; disconnect → deterministic fallback
- ☐ Language: fix language to German, dictate German → correct text

## 4. Hotkeys
- ☐ Rebind each action in Settings; conflicts reported as warnings not crashes
- ☐ macOS: hotkeys work with NO Input Monitoring permission
- ☐ KDE Wayland: portal dialog appears once; press & release both delivered
- ☐ GNOME Wayland (48+): portal path; older GNOME: evdev fallback works after
  joining `input` group (doctor verifies)
- ☐ X11: grabs work; releasing another app's grab conflict is reported

## 5. TTS (read selection)
- ☐ Select text in browser → hotkey → Kokoro speaks < ~0.5 s (warm)
- ☐ Long article: playback starts while later sentences still synthesize
- ☐ Stop hotkey halts immediately
- ☐ Rate 0.5×/2.0× audible difference; voice switch works (af_heart / bf_emma)
- ☐ Kokoro model deleted → Piper (opt) → system voice; each announces in log
- ☐ Selection intact after clipboard-fallback capture (clipboard restored)
- ☐ Dictating while speaking stops playback first

## 6. Agent behavior
- ☐ Tray icon reflects idle/listening/processing live
- ☐ Mode submenu radio state follows changes from menu AND settings
- ☐ Settings window close = hide (agent keeps running); single-instance:
  second launch focuses settings instead
- ☐ Launch at login (macOS LaunchAgent / Linux autostart) on & off
- ☐ Idle unload: after timeout, RAM drops; next dictation reloads transparently

## 7. Privacy verification (each release)
- ☐ `rg -n "http" crates apps --type rust` shows only huggingface (models.rs)
  and 127.0.0.1 (ollama)
- ☐ Run with `lsof -i` / `ss -tup` during dictation+TTS: zero sockets
  (except localhost when Ollama tier explicitly enabled)
- ☐ Offline build compiles: `cargo build --no-default-features
  --features stt-whisper,llm-llama,tts-kokoro`
- ☐ Fully Offline mode + no models → actionable ModelMissing errors, no network

## 8. Packaging
- ☐ macOS: `scripts/macos-sign-notarize.sh` → notarized, stapled; Gatekeeper
  clean on a fresh machine; permissions prompts show app name & usage string
- ☐ Linux: AppImage runs on Ubuntu LTS + Fedora latest; `.deb` installs deps;
  `--doctor` correct on both
- ☐ Flatpak (opt): portal hotkeys + AT-SPI2 insertion work; typing rungs
  correctly reported unavailable
