import CoreML
import FluidAudio
import Foundation

actor Dictation {
    private static let minimumSeconds = 0.3
    private static let previewInterval: Duration = .milliseconds(400)
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
        endRecording()
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
        endRecording()
        let audio = try AudioConverter().resampleAudioFile(file)
        let recorder = Recorder()
        recorder.replay(audio, sampleRate: 16_000)
        self.recorder = recorder
        preview = Task { await self.streamPreview(of: recorder) }
        try await Task.sleep(for: .seconds(Double(audio.count) / 16_000 + 0.2))
        await stop()
    }

    func cancel() {
        endRecording()
        Output.send(["type": "cancelled"])
    }

    @discardableResult
    private func endRecording() -> (samples: [Float], sampleRate: Double)? {
        preview?.cancel()
        preview = nil
        guard let recorder else { return nil }
        self.recorder = nil
        return recorder.stop()
    }

    private func streamPreview(of recorder: Recorder) async {
        var shown = ""
        while !Task.isCancelled {
            try? await Task.sleep(for: Self.previewInterval)
            guard self.recorder === recorder, !Task.isCancelled else { return }
            let captured = recorder.snapshot()
            guard Double(captured.samples.count) / captured.sampleRate >= Self.minimumSeconds,
                let text = try? await transcribe(captured.samples, sampleRate: captured.sampleRate),
                self.recorder === recorder, !Task.isCancelled, text != shown
            else { continue }
            shown = text
            Output.send(["type": "partial", "text": text])
        }
    }

    func stop() async {
        guard asr != nil, let captured = endRecording() else {
            Output.send(["type": "transcript", "text": ""])
            return
        }
        guard Double(captured.samples.count) / captured.sampleRate >= Self.minimumSeconds else {
            Output.send(["type": "transcript", "text": ""])
            return
        }
        do {
            let text = try await transcribe(captured.samples, sampleRate: captured.sampleRate)
            Output.send(["type": "transcript", "text": text])
        } catch {
            Output.failure("transcribe", error.localizedDescription)
        }
    }

    func transcribe(file: URL) async throws -> String {
        guard asr != nil else { throw ASRError.notInitialized }
        return try await transcribe(AudioConverter().resampleAudioFile(file), sampleRate: 16_000)
    }

    private func transcribe(_ captured: [Float], sampleRate: Double) async throws -> String {
        guard let asr else { throw ASRError.notInitialized }
        let samples =
            sampleRate == 16_000 ? captured : try AudioConverter().resample(captured, from: sampleRate)
        var decoderState = try TdtDecoderState()
        let result = try await asr.transcribe(samples, decoderState: &decoderState)
        return result.text.trimmingCharacters(in: .whitespacesAndNewlines)
    }
}
