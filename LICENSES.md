# Licenses

Local AI Flow itself: **MIT** (see `LICENSE`).

## Bundled / vendored source

| Component | License | Notes |
|---|---|---|
| `crates/laf-kokoro` (vendored from kokoroxide 0.1.5) | MIT OR Apache-2.0 | ported to ort 2.x; LICENSE files preserved in the crate dir |

## Models (downloaded on explicit user action; never bundled in the repo)

| Model | License |
|---|---|
| Whisper ggml conversions (ggerganov/whisper.cpp) | MIT |
| Qwen2.5-3B-Instruct GGUF | Qwen Research License (review for commercial use) |
| Qwen2.5-1.5B-Instruct GGUF | Apache-2.0 |
| Kokoro-82M v1.0 ONNX + voices (onnx-community) | Apache-2.0 |
| Piper voices (optional, user-supplied) | MIT (engine); per-voice licensing varies |

## Key Rust dependencies

| Crate | License | Role |
|---|---|---|
| tauri 2 (+ tauri-build, plugins autostart/single-instance) | Apache-2.0 OR MIT | app shell |
| whisper-rs 0.16 | Unlicense (whisper.cpp itself: MIT) | STT bindings |
| llama-cpp-2 0.1.146 | MIT OR Apache-2.0 (llama.cpp: MIT) | LLM bindings |
| ort 2.0.0-rc.12 (+ ONNX Runtime binaries) | MIT OR Apache-2.0 (ONNX Runtime: MIT) | Kokoro inference |
| cpal 0.18 | Apache-2.0 | audio I/O |
| global-hotkey 0.8 | Apache-2.0 OR MIT | macOS/X11 hotkeys |
| ashpd 0.13 | MIT | XDG portals (GlobalShortcuts) |
| atspi 0.30 (odilia) | Apache-2.0 OR MIT | AT-SPI2 insertion |
| zbus 5 | MIT | D-Bus |
| x11rb 0.13 | MIT OR Apache-2.0 | XTEST |
| evdev 0.13 | MIT OR Apache-2.0 | raw input fallback |
| arboard 3 | MIT OR Apache-2.0 | clipboards (incl. PRIMARY) |
| objc2 / objc2-app-kit / objc2-foundation | MIT | macOS AppKit FFI |
| core-foundation 0.10 / core-graphics 0.25 | MIT OR Apache-2.0 | macOS CF/CG FFI |
| reqwest 0.12 (rustls) | MIT OR Apache-2.0 | model downloads only |
| tokio, serde, serde_json, thiserror, anyhow, tracing, regex, crossbeam-channel, sha2, hex, dirs, futures-util, encoding_rs | MIT and/or Apache-2.0 | plumbing |

The full transitive set with exact versions is pinned in `Cargo.lock`; audit
with `cargo license` or `cargo deny` if you need a machine-checked report.

## External tools invoked at runtime (user-installed, all optional)

| Tool | License | Used for |
|---|---|---|
| wtype | MIT | Wayland typing (wlroots) |
| ydotool (+ ydotoold) | AGPL-3.0 (separate process; invoked as a CLI) | Wayland typing via uinput |
| xdotool | BSD-3-Clause | X11 unicode typing |
| espeak-ng (libespeak-ng) | GPL-3.0 (dynamically linked system library) | Kokoro G2P + last-resort voice |
| speech-dispatcher (spd-say) | GPL-2.0+ (separate process) | Linux system-voice fallback |
| piper | MIT | alternate neural TTS (subprocess) |
| macOS `say` | Apple OS component | macOS last-resort voice |

Note on espeak-ng (GPL-3.0): `laf-kokoro` links `libespeak-ng` dynamically.
Distributing combined binaries must respect the GPL — the AppImage/deb we
describe list espeak-ng as an external dependency (recommends), keeping it a
system library supplied by the distribution, and all our own code is
MIT/Apache-2.0 licensed (GPL-compatible). If you need to avoid even dynamic
GPL linkage, build without `tts-kokoro` and use Piper (MIT) instead.
