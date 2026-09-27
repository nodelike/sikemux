import Foundation

@main
struct SikemuxVoice {
    static func main() async {
        if CommandLine.arguments.contains("--version") {
            print("sikemux-voice 1")
            return
        }
        let dictation = Dictation()
        if let file = option("--transcribe") {
            await transcribe(file: file, with: dictation)
            return
        }
        if let file = option("--stream") {
            await stream(file: file, with: dictation)
            return
        }
        let lines = AsyncStream<String> { continuation in
            Thread.detachNewThread {
                while let line = readLine() { continuation.yield(line) }
                continuation.finish()
            }
        }
        for await line in lines {
            guard let data = line.data(using: .utf8),
                let command = try? JSONDecoder().decode(Command.self, from: data)
            else { continue }
            switch command.type {
            case "prepare":
                await dictation.prepare(modelsDir: command.modelsDir ?? "")
            case "start":
                await dictation.start(vocabulary: command.vocabulary ?? [])
            case "stop":
                await dictation.stop()
            case "cancel":
                await dictation.cancel()
            default:
                Output.failure("protocol", "Unknown command \(command.type)")
            }
        }
    }

    private static func option(_ name: String) -> String? {
        let arguments = CommandLine.arguments
        guard let index = arguments.firstIndex(of: name), index + 1 < arguments.count else { return nil }
        return arguments[index + 1]
    }

    private static func prepareForFile(_ dictation: Dictation) async -> [String] {
        guard let models = option("--models"), await dictation.prepare(modelsDir: models) else {
            FileHandle.standardError.write(
                Data("usage: sikemux-voice --transcribe|--stream <audio> --models <dir> [--vocabulary a,b]\n".utf8))
            exit(2)
        }
        return option("--vocabulary")?.split(separator: ",").map(String.init) ?? []
    }

    private static func stream(file: String, with dictation: Dictation) async {
        let vocabulary = await prepareForFile(dictation)
        do {
            try await dictation.stream(file: URL(fileURLWithPath: file), vocabulary: vocabulary)
        } catch {
            Output.failure("transcribe", error.localizedDescription)
            exit(1)
        }
    }

    private static func transcribe(file: String, with dictation: Dictation) async {
        let vocabulary = await prepareForFile(dictation)
        do {
            let started = Date()
            let text = try await dictation.transcribe(file: URL(fileURLWithPath: file), vocabulary: vocabulary)
            Output.send(["type": "transcript", "text": text, "seconds": Date().timeIntervalSince(started)])
        } catch {
            Output.failure("transcribe", error.localizedDescription)
            exit(1)
        }
    }
}
