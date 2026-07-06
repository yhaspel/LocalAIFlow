// laf-whisperkit-helper — ANE-accelerated Whisper for Local AI Flow (M6).
//
// Protocol: raw little-endian Float32 16 kHz mono PCM on stdin.
// Emits line-delimited JSON on stdout: {"partial": "..."} during streaming,
// {"final": "..."} after stdin closes. All processing is on-device.
//
// Usage: laf-whisperkit-helper [--model-path /path/to/whisperkit-model-dir]
// Without --model-path, WhisperKit resolves its default local model folder
// (~/Documents/huggingface/models/argmaxinc/whisperkit-coreml/...); model
// download, if needed, is WhisperKit's own explicit behavior — pre-download
// with `whisperkit-cli download` for fully-offline use.

import Foundation
import WhisperKit

struct Emit {
    static func line(_ key: String, _ text: String) {
        let obj: [String: String] = [key: text]
        if let data = try? JSONSerialization.data(withJSONObject: obj),
           let s = String(data: data, encoding: .utf8) {
            print(s)
            FileHandle.standardOutput.synchronizeFile()
            fflush(stdout)
        }
    }
}

@main
struct Main {
    static func main() async {
        var modelPath: String? = nil
        var args = ArraySlice(CommandLine.arguments.dropFirst())
        while let a = args.popFirst() {
            if a == "--model-path", let p = args.popFirst() { modelPath = p }
        }

        let pipe: WhisperKit
        do {
            var config = WhisperKitConfig()
            if let modelPath { config.modelFolder = modelPath }
            pipe = try await WhisperKit(config)
        } catch {
            FileHandle.standardError.write(Data("failed to load WhisperKit: \(error)\n".utf8))
            exit(2)
        }

        // Accumulate stdin PCM; re-transcribe a sliding window for partials
        // (same strategy as the portable whisper.cpp engine).
        var samples: [Float] = []
        samples.reserveCapacity(16_000 * 30)
        let stdinHandle = FileHandle.standardInput
        var lastDecode = Date.distantPast
        var pendingSinceDecode = 0

        while true {
            let chunk = stdinHandle.availableData
            if chunk.isEmpty { break } // EOF
            chunk.withUnsafeBytes { (raw: UnsafeRawBufferPointer) in
                let floats = raw.bindMemory(to: Float32.self)
                samples.append(contentsOf: floats)
                pendingSinceDecode += floats.count
            }
            let enoughNew = pendingSinceDecode >= 8_000 // 0.5 s
            let longEnough = samples.count >= 6_400     // 0.4 s
            if longEnough && enoughNew && Date().timeIntervalSince(lastDecode) > 0.4 {
                pendingSinceDecode = 0
                lastDecode = Date()
                if let text = await transcribe(pipe, samples) {
                    Emit.line("partial", text)
                }
            }
        }
        if let text = await transcribe(pipe, samples) {
            Emit.line("final", text)
        } else {
            Emit.line("final", "")
        }
    }

    static func transcribe(_ pipe: WhisperKit, _ samples: [Float]) async -> String? {
        guard !samples.isEmpty else { return nil }
        do {
            let results = try await pipe.transcribe(audioArray: samples)
            return results.map(\.text).joined(separator: " ")
                .trimmingCharacters(in: .whitespacesAndNewlines)
        } catch {
            FileHandle.standardError.write(Data("transcribe error: \(error)\n".utf8))
            return nil
        }
    }
}
