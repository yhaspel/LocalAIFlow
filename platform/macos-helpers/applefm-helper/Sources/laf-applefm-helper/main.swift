// laf-applefm-helper — Apple Foundation Models cleanup tier (M6, macOS 26+).
//
// Protocol: one JSON object {"system": "...", "user": "..."} on stdin;
// the formatted text on stdout. Uses Apple's ON-DEVICE language model via
// FoundationModels; no network. Requires Apple Intelligence to be enabled.

import Foundation
import FoundationModels

struct Input: Decodable {
    let system: String
    let user: String
}

@main
struct Main {
    static func main() async {
        let data = FileHandle.standardInput.readDataToEndOfFile()
        guard let input = try? JSONDecoder().decode(Input.self, from: data) else {
            FileHandle.standardError.write(Data("invalid input JSON\n".utf8))
            exit(2)
        }
        let model = SystemLanguageModel.default
        guard case .available = model.availability else {
            FileHandle.standardError.write(
                Data("on-device model unavailable (enable Apple Intelligence)\n".utf8))
            exit(3)
        }
        do {
            let session = LanguageModelSession(instructions: input.system)
            let response = try await session.respond(to: input.user)
            print(response.content)
        } catch {
            FileHandle.standardError.write(Data("generation failed: \(error)\n".utf8))
            exit(4)
        }
    }
}
