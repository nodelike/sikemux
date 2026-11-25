import CoreML
import FluidAudio
import Foundation

actor Dictation {
    private static let minimumSeconds = 0.3
    private static let minimumSamples = Int(minimumSeconds * Recorder.targetSampleRate)
    private static let previewInterval: Duration = .milliseconds(400)
    private static let previewWindowSamples = 15 * Int(Recorder.targetSampleRate)
    private static let previewCommitStride = 12 * Int(Recorder.targetSampleRate)
    private static let previewMinNewSamples = Int(0.25 * Recorder.targetSampleRate)
    /// The app downloads and verifies this folder before asking for it; the helper never fetches models.
    private static let asrFolder = "parakeet-tdt-0.6b-v3"

    private var asr: AsrManager?
    private var recorder: Recorder?
    private var preview: Task<Void, Never>?

    @discardableResult
    func prepare(modelsDir: String) async -> Bool {
        if asr != nil {
            Output.send(["type": "ready"])
            return true
        }
        let root = URL(fileURLWithPath: modelsDir, isDirectory: true)
        do {
            let models = try Self.loadAsrModels(
                from: root.appendingPathComponent(Self.asrFolder, isDirectory: true))
            let manager = AsrManager()
            try await manager.loadModels(models)
            self.asr = manager
            Output.send(["type": "ready"])
            return true
        } catch {
            Output.failure("models", error.localizedDescription)
            return false
        }
    }

    private static func loadAsrModels(from directory: URL) throws -> AsrModels {
        let names = ModelNames.ASR.self
        let parts: [(file: String, units: MLComputeUnits)] = [
            (names.preprocessorFile, .cpuOnly),
            (names.encoderFile, .cpuAndNeuralEngine),
            (names.decoderFile, .cpuAndNeuralEngine),
            (names.jointV3File, .cpuAndNeuralEngine),
        ]
        var loaded: [MLModel] = []
        for (index, part) in parts.enumerated() {
            Output.progress(stage: "compile", fraction: Double(index) / Double(parts.count))
            let configuration = MLModelConfigurationUtils.defaultConfiguration(computeUnits: part.units)
            loaded.append(
                try MLModel(contentsOf: directory.appendingPathComponent(part.file), configuration: configuration))
        }
        Output.progress(stage: "compile", fraction: 1)
        let tokens = try JSONDecoder().decode(
            [String: String].self,
            from: Data(contentsOf: directory.appendingPathComponent(names.vocabularyFile)))
        let vocabulary = Dictionary(
            uniqueKeysWithValues: tokens.compactMap { key, token in Int(key).map { ($0, token) } })
        return AsrModels(
            encoder: loaded[1], preprocessor: loaded[0], decoder: loaded[2], joint: loaded[3],
            configuration: AsrModels.defaultConfiguration(), vocabulary: vocabulary, version: .v3)
    }

    func start() async {
        guard asr != nil else {
            Output.failure("models", "The speech model is not loaded yet.")
            return
        }
        await endRecording()
        do {
            try await Recorder.ensurePermission()
            let recorder = Recorder()
            try recorder.start()
            self.recorder = recorder
            Output.send(["type": "listening"])
            preview = Task { await self.streamPreview(of: recorder) }
        } catch RecorderError.microphoneDenied {
            Output.failure("microphone", RecorderError.microphoneDenied.localizedDescription)
        } catch {
            Output.failure("audio", error.localizedDescription)
        }
    }

    /// Plays a file in as if it were being spoken, so streaming can be checked without a microphone.
    func stream(file: URL) async throws {
        guard asr != nil else { throw ASRError.notInitialized }
        await endRecording()
        let audio = try await Self.resampleFile(file)
        let recorder = Recorder()
        recorder.replay(audio)
        self.recorder = recorder
        preview = Task { await self.streamPreview(of: recorder) }
        try await Task.sleep(for: .seconds(Double(audio.count) / Recorder.targetSampleRate + 0.2))
        await stop()
    }

    func cancel() async {
        await endRecording()
        Output.send(["type": "cancelled"])
    }

    @discardableResult
    private func endRecording() async -> [Float]? {
        let runningPreview = preview
        preview = nil
        runningPreview?.cancel()
        let captured = recorder?.stop()
        self.recorder = nil
        await runningPreview?.value
        return captured
    }

    private func streamPreview(of recorder: Recorder) async {
        var committedOffset = 0
        var committedText = ""
        var lastDecodedCount = 0
        var shown = ""
        while !Task.isCancelled {
            try? await Task.sleep(for: Self.previewInterval)
            guard self.recorder === recorder, !Task.isCancelled else { return }
            let total = recorder.sampleCount()
            guard total >= Self.minimumSamples, total - lastDecodedCount >= Self.previewMinNewSamples else {
                continue
            }
            while total - committedOffset >= Self.previewWindowSamples {
                let (chunk, _) = recorder.slice(from: committedOffset, maxCount: Self.previewCommitStride)
                guard chunk.count == Self.previewCommitStride else { break }
                guard let chunkText = try? await transcribe(chunk),
                    self.recorder === recorder, !Task.isCancelled
                else { return }
                committedText = Self.joinText(committedText, chunkText)
                committedOffset += chunk.count
            }
            let (tail, _) = recorder.slice(from: committedOffset, maxCount: Self.previewWindowSamples)
            lastDecodedCount = committedOffset + tail.count
            let combined: String
            if tail.count >= Self.minimumSamples {
                guard let tailText = try? await transcribe(tail),
                    self.recorder === recorder, !Task.isCancelled
                else { continue }
                combined = Self.joinText(committedText, tailText)
            } else {
                combined = committedText
            }
            guard combined != shown else { continue }
            shown = combined
            Output.send(["type": "partial", "text": combined])
        }
    }

    func stop() async {
        guard asr != nil, let captured = await endRecording() else {
            Output.send(["type": "transcript", "text": ""])
            return
        }
        guard captured.count >= Self.minimumSamples else {
            Output.send(["type": "transcript", "text": ""])
            return
        }
        do {
            let text = try await transcribe(captured)
            Output.send(["type": "transcript", "text": text])
        } catch {
            Output.failure("transcribe", error.localizedDescription)
        }
    }

    func transcribe(file: URL) async throws -> String {
        guard asr != nil else { throw ASRError.notInitialized }
        let samples = try await Self.resampleFile(file)
        return try await transcribe(samples)
    }

    private static func resampleFile(_ file: URL) async throws -> [Float] {
        try await Task.detached(priority: .userInitiated) {
            try AudioConverter().resampleAudioFile(file)
        }.value
    }

    private static func joinText(_ first: String, _ second: String) -> String {
        if first.isEmpty { return second }
        if second.isEmpty { return first }
        return "\(first) \(second)"
    }

    private func transcribe(_ samples: [Float]) async throws -> String {
        guard let asr else { throw ASRError.notInitialized }
        var decoderState = try TdtDecoderState()
        let result = try await asr.transcribe(samples, decoderState: &decoderState, language: .english)
        return result.text.trimmingCharacters(in: .whitespacesAndNewlines)
    }
}
