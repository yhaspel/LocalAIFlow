# Local AI Flow

Privacy-first, **fully on-device** AI dictation for macOS and Linux — press a
hotkey, speak into any app, and clean, correctly formatted text is inserted
into the focused field. Plus a feature Wispr Flow doesn't have: press a second
hotkey and the currently **selected text is read aloud** with a local neural
voice.

* STT: whisper.cpp (`large-v3-turbo` quantized by default), streaming partials
* Cleanup: filler removal, punctuation, capitalization, per-mode formatting —
  deterministic tier always available, small local LLM tier (Qwen2.5 via
  llama.cpp) when installed, optional local Ollama
* Modes: Raw · Auto · Email · Message · List · Code · Command (spoken editing
  commands: "new line", "period", "delete that", …)
* TTS: Kokoro-82M (Apache-2.0) via ONNX Runtime, streamed sentence-by-sentence;
  Piper and the OS voice as fallbacks
* Tray/menu-bar agent, floating HUD with live waveform + partial transcript,
  push-to-talk **and** toggle hotkeys, custom dictionary, multi-language

---

## Privacy — exactly what touches the network

**Inference never touches the network.** No audio, transcript, generated text,
or anything derived from them ever leaves the machine. There is no telemetry,
no analytics, no crash reporting, no update pinging. Grep the source: the
workspace contains exactly **one** network code path:

* `crates/laf-core/src/models.rs` (`ModelManager::download`, behind the
  `online` cargo feature): downloads model weights from `huggingface.co` over
  HTTPS **only when you click Download** (or the onboarding button). Every
  file's SHA-256 and size are pinned in the source and verified during
  streaming; mismatches are deleted.

The only other socket the app can open is to `127.0.0.1:11434` **if** you
switch the cleanup engine to "Ollama on this machine" — a server you run
yourself; the client refuses non-loopback URLs at construction time.

Fully Offline operation, two layers:

1. **Runtime toggle** (Settings → "Fully Offline mode", or the onboarding
   "I'll stay fully offline" button): `ModelManager` refuses all downloads.
   Use pre-bundled models (place them under the app's `resources/models/`, or
   drop files into the models folder shown in Settings → Debug).
2. **Offline build**: `cargo build --release --no-default-features
   --features stt-whisper,llm-llama,tts-kokoro` produces a binary with the
   download code (and reqwest) **compiled out entirely**.

## Architecture

```
crates/laf-core            shared, OS-agnostic core
  ├─ traits.rs             AudioCapture · VoiceActivityDetector · SpeechToText/SttSession
  │                        TextCleaner · TextInserter · SelectionReader
  │                        SpeechSynthesizer/TtsPlayback · HotkeyBackend
  ├─ pipeline.rs           Idle→Listening→Processing→Inserting state machine
  ├─ clean/                deterministic tier: fillers, punctuation, spoken commands
  ├─ modes.rs              per-mode LLM prompts + deterministic post-format
  ├─ dictionary.rs         custom dictionary (+ STT vocabulary hints)
  ├─ models.rs             registry (pinned SHA-256) + download/verify/delete
  ├─ settings.rs · vad.rs · resample.rs · metrics.rs · doctor.rs · hotkeys.rs
crates/laf-engines         portable engines (both OSes)
  ├─ audio.rs              cpal capture → 16 kHz mono; PCM playback sink
  ├─ stt_whisper.rs        whisper.cpp streaming (Metal/CoreML/CUDA/Vulkan features)
  ├─ clean_llama.rs        llama.cpp GGUF cleaner (chat-template aware)
  ├─ clean_ollama.rs       loopback-only Ollama client
  ├─ tts_kokoro.rs         Kokoro engine (uses crates/laf-kokoro)
  ├─ tts_piper.rs          Piper subprocess fallback
  └─ tts_system.rs         `say` / speech-dispatcher / espeak-ng last resort
crates/laf-kokoro          vendored kokoroxide port on ort 2.x (see below)
crates/laf-platform-linux  AT-SPI2 · wtype/ydotool · XTEST · clipboard chain;
                           portal/evdev/X11 hotkeys; Linux doctor
crates/laf-platform-macos  AX · CGEvent unicode · pasteboard-⌘V chain;
                           Carbon hotkeys; permissions; macOS doctor
apps/desktop/src-tauri     Tauri v2 shell: tray agent, HUD + Settings webviews
apps/desktop/ui            TypeScript webviews (no framework, no runtime deps)
platform/macos-helpers     OPTIONAL Swift helpers (WhisperKit / Foundation Models)
```

Every OS-specific capability sits behind a trait with a **real implementation
on each OS** — the pipeline, cleanup, settings, and model manager are shared.

### Text insertion chains (and their trade-offs)

macOS (`laf-platform-macos/src/inserter.rs`):
1. AX `AXSelectedText` write on the focused element — no side effects
2. CGEvent unicode typing (`CGEventKeyboardSetUnicodeString`) — layout-proof
3. NSPasteboard + synthetic ⌘V, clipboard restored after ~600 ms

Linux (`laf-platform-linux/src/inserter.rs`), session detected at runtime:
1. AT-SPI2 `EditableText.InsertText` on the focused widget (X11 & Wayland)
2. Synthetic typing — Wayland: `wtype` (zwp_virtual_keyboard_v1; wlroots) →
   `ydotool` (kernel uinput; works on GNOME/KDE, needs ydotoold);
   X11: `xdotool type`
3. Clipboard + paste chord — Wayland: wtype/ydotool chord; X11: native XTEST
   (no external tool), clipboard restored afterwards

### Hotkeys

* macOS: Carbon `RegisterEventHotKey` via the `global-hotkey` crate —
  delivers press **and** release (push-to-talk), and needs **no Input
  Monitoring permission** (that's only required by event taps, which we don't
  use).
* Linux X11: `XGrabKey` (same crate), press + release.
* Linux Wayland: `org.freedesktop.portal.GlobalShortcuts` (KDE; GNOME 48+) —
  Activated/Deactivated signals give push-to-talk; falls back to raw evdev
  (`input` group required) when the portal is missing. `--doctor` tells you
  exactly which path is active.

## Building

Prerequisites (both): Rust 1.82+, `cmake`, a C/C++ toolchain.

**Linux** (Debian/Ubuntu names):
```sh
sudo apt install build-essential cmake pkg-config \
  libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev \
  libasound2-dev libespeak-ng-dev
cargo build --release            # or: cargo tauri build (bundles AppImage + deb)
```

**macOS** (13+, Apple Silicon or Intel):
```sh
xcode-select --install
brew install cmake espeak-ng
cargo build --release            # whisper.cpp builds with Metal by default
./scripts/make-icns.sh && cargo tauri build   # .app / .dmg
```

Optional GPU features: `--features cuda` or `--features vulkan` (Linux),
`--features whisper-coreml` (macOS, adds the CoreML encoder build).

Run: `target/release/local-ai-flow` — the tray icon appears; first run opens
Settings with onboarding (permissions + model download). Check your
environment any time with:
```sh
local-ai-flow --doctor
```

ONNX Runtime note: building with the default `tts-kokoro` feature downloads
the onnxruntime **build-time** static libs via the `ort` crate (pyke CDN) —
that's a build-machine concern, not an app-runtime one. Those prebuilt libs
target glibc ≥ 2.38 / gcc-13 (Ubuntu 24.04+, Fedora 39+, Debian 13+). On
older distros (e.g. Ubuntu 22.04) either build without `tts-kokoro`
(Piper/system TTS still work) or use `--features laf-kokoro/load-dynamic`
to dlopen a distro/system onnxruntime at runtime instead.

## Models

| Model | Size | License | Used for |
|---|---|---|---|
| Whisper large-v3-turbo Q5_0 (default) | 574 MB | MIT | STT |
| Whisper small/base/tiny Q5_1 | 190/60/32 MB | MIT | STT (low-end) |
| Qwen2.5-3B-Instruct Q4_K_M (default) | 2.1 GB | Qwen Research License¹ | cleanup |
| Qwen2.5-1.5B-Instruct Q4_K_M | 1.1 GB | Apache-2.0 | cleanup (low-end) |
| Kokoro-82M v1.0 ONNX (quantized) + voices | 93 MB | Apache-2.0 | TTS |

¹ Review the Qwen Research License for commercial use; the 1.5B model is
Apache-2.0 if you need a fully permissive stack.

Storage: `~/Library/Application Support/LocalAIFlow/models` (macOS),
`$XDG_DATA_HOME/LocalAIFlow/models` (Linux, default `~/.local/share/…`).
Bundled-model installs are read from the app's `resources/models/`.

## Linux runtime notes (Wayland reality, verbatim honesty)

Wayland deliberately restricts synthetic input. What that means here:

* **KDE / GNOME Wayland typing** needs `ydotool` + its daemon:
  `systemctl --user enable --now ydotool`, plus uinput access —
  `sudo cp packaging/99-localaiflow-uinput.rules /etc/udev/rules.d/ &&
  sudo usermod -aG input $USER` (re-login). Without it, insertion still works
  via AT-SPI2 (where apps support it) or clipboard-paste.
* **Sway/Hyprland** get first-class typing via `wtype`.
* **AT-SPI2** requires accessibility enabled (default on GNOME; on others:
  `gsettings set org.gnome.desktop.interface toolkit-accessibility true`).
  Electron apps expose it only with `--force-renderer-accessibility`.
* **Hotkeys**: portal (KDE, GNOME 48+) or evdev fallback (`input` group).
* `scripts/linux-setup.sh` automates all of the above; `--doctor` verifies.
* **Flatpak**: the sandbox blocks uinput/evdev — portal hotkeys + AT-SPI2 +
  clipboard still work, but prefer the AppImage/deb for full capability
  (details in `packaging/flatpak/dev.localaiflow.app.yml`).

## macOS permissions (what & why)

| Permission | Why | When asked |
|---|---|---|
| Microphone | dictation audio (processed locally) | first dictation (OS prompt) |
| Accessibility | AX insertion + synthetic keystrokes/⌘V + selection reading | onboarding button → System Settings |
| Input Monitoring | **not required** (no event taps; Carbon hotkeys) | never |

Distribution is Developer ID (Hardened Runtime, notarized) — **not** the App
Store sandbox, which forbids cross-app AX insertion. See
`scripts/macos-sign-notarize.sh`; identities/credentials come from env vars.

## Performance

Local-only latency instrumentation lives under Settings → Debug (p50/p95 per
stage: `stt_finalize`, `clean`, `insert`, `e2e_stop_to_insert`,
`tts_first_dispatch`). Targets: end-to-end stop→inserted < 1 s for a sentence
on a modern laptop (large-v3-turbo Q5, Metal/AVX2); HUD partials < 300 ms
after speech start (adaptive decode cadence); TTS first audio < 500 ms
(sentence-chunked Kokoro). Idle models unload after a configurable timeout.

## Documented deviations from the original plan

* **kokoroxide is vendored** (`crates/laf-kokoro`, MIT/Apache-2.0 preserved):
  the published crate depends on `ort ^1.16`, which is fully yanked from
  crates.io, making it uninstallable. The vendored copy is ported to
  maintained `ort` 2.x; G2P/phoneme/tokenizer/voice logic is verbatim.
* **No Input Monitoring requirement on macOS**: hotkeys use Carbon
  `RegisterEventHotKey` (press+release included) instead of a CGEventTap —
  fewer scary permissions, same functionality. If a future feature needs
  taps, the permission helpers are already in `permissions.rs`.

## Development

```sh
cargo test -p laf-core          # deterministic cleaner, dictionary, VAD,
                                # resampler, models, pipeline state machine
cargo check --workspace         # Linux host
cargo check -p laf-platform-macos --target aarch64-apple-darwin
```

Manual QA matrix: `docs/QA-CHECKLIST.md`. Dependency & model licenses:
`LICENSES.md`. UI sources: `apps/desktop/ui/src` (`npm run build` regenerates
`dist/`, which is committed so no Node is needed to build the app).
