# Optional macOS-native acceleration helpers (M6)

These two small Swift executables are an **optional performance tier** for
Apple Silicon. The app runs fully without them (portable whisper.cpp with
Metal + llama.cpp); when the binaries exist next to `local-ai-flow` (or in
`Local AI Flow.app/Contents/MacOS/`), they are auto-detected at runtime and
used behind the same `SpeechToText` / `TextCleaner` traits, with clean
fallback if they fail.

| Helper | Uses | Requires |
|---|---|---|
| `laf-whisperkit-helper` | [WhisperKit](https://github.com/argmaxinc/WhisperKit) (ANE-accelerated Whisper); on macOS 26+ you can swap in `SpeechAnalyzer`/`SpeechTranscriber` | macOS 14+, Apple Silicon, Xcode 15+ |
| `laf-applefm-helper` | Apple Foundation Models (`LanguageModelSession`) for the cleanup step | macOS 26+, Apple Intelligence enabled |

Everything stays on-device: WhisperKit models are local CoreML bundles;
Foundation Models is Apple's on-device LLM. Neither helper opens a network
connection (WhisperKit's model download, if you use it, is its own explicit
step — pass `--model-path` to use a pre-downloaded model).

## Build

```sh
cd platform/macos-helpers/whisperkit-helper
swift build -c release
cp .build/release/laf-whisperkit-helper "/Applications/Local AI Flow.app/Contents/MacOS/"

cd ../applefm-helper
swift build -c release   # requires the macOS 26 SDK
cp .build/release/laf-applefm-helper "/Applications/Local AI Flow.app/Contents/MacOS/"
```

Then in Local AI Flow settings: STT engine → `whisperkit`, or Cleanup engine →
`Apple Foundation Models`.

## Protocols (stdin/stdout, line-delimited JSON)

* **whisperkit-helper**: raw little-endian f32 16 kHz mono PCM on stdin;
  emits `{"partial":"…"}` lines while audio streams and `{"final":"…"}` on
  stdin EOF, then exits.
* **applefm-helper**: one JSON object `{"system":"…","user":"…"}` on stdin;
  writes the formatted text to stdout and exits.
