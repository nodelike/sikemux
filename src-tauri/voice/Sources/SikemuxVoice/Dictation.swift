import CoreML
import FluidAudio
import Foundation

actor Dictation {
    private static let minimumSeconds = 0.3
    private static let previewInterval: Duration = .milliseconds(400)
    /// The app downloads and verifies these folders before asking for them; the helper never fetches models.
    private static let asrFolder = "parakeet-tdt-0.6b-v3"
    private static let ctcFolder = "parakeet-ctc-110m-coreml"

    private var asr: AsrManager?
    private var spotter: CtcKeywordSpotter?
    private var ctcDirectory: URL?
    private var tokenizer: CtcTokenizer?
    private var boosting: (terms: [String], context: CustomVocabularyContext, rescorer: VocabularyRescorer)?
    private var recorder: Recorder?
    private var preview: Task<Void, Never>?
    private var vocabulary: [String] = []

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

            Output.progress(stage: "vocabulary", fraction: 0)
            let ctcDirectory = root.appendingPathComponent(Self.ctcFolder, isDirectory: true)
            let ctcModels = try await CtcModels.loadDirect(from: ctcDirectory)
            let tokenizer = try await CtcTokenizer.load(from: ctcDirectory)
            Output.progress(stage: "vocabulary", fraction: 1)

            self.asr = manager
            self.spotter = CtcKeywordSpotter(models: ctcModels, blankId: ctcModels.vocabulary.count)
            self.ctcDirectory = ctcDirectory
            self.tokenizer = tokenizer
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

    func start(vocabulary: [String]) async {
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
            self.vocabulary = vocabulary
            Output.send(["type": "listening"])
            preview = Task { await self.streamPreview(of: recorder) }
        } catch RecorderError.microphoneDenied {
            Output.failure("microphone", RecorderError.microphoneDenied.localizedDescription)
        } catch {
            Output.failure("audio", error.localizedDescription)
        }
    }

    /// Plays a file in as if it were being spoken, so streaming can be checked without a microphone.
    func stream(file: URL, vocabulary: [String]) async throws {
        guard asr != nil else { throw ASRError.notInitialized }
        endRecording()
        let audio = try AudioConverter().resampleAudioFile(file)
        let recorder = Recorder()
        recorder.replay(audio, sampleRate: 16_000)
        self.recorder = recorder
        self.vocabulary = vocabulary
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
                let text = try? await transcribe(captured.samples, sampleRate: captured.sampleRate, boost: false),
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

    func transcribe(file: URL, vocabulary: [String]) async throws -> String {
        guard asr != nil else { throw ASRError.notInitialized }
        self.vocabulary = vocabulary
        return try await transcribe(AudioConverter().resampleAudioFile(file), sampleRate: 16_000)
    }

    private func transcribe(_ captured: [Float], sampleRate: Double, boost: Bool = true) async throws -> String {
        guard let asr else { throw ASRError.notInitialized }
        let samples =
            sampleRate == 16_000 ? captured : try AudioConverter().resample(captured, from: sampleRate)
        var decoderState = try TdtDecoderState()
        let result = try await asr.transcribe(samples, decoderState: &decoderState)
        let text = boost ? await boosted(result, samples: samples) : result.text
        return text.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private func boosted(_ result: ASRResult, samples: [Float]) async -> String {
        guard !vocabulary.isEmpty, let spotter,
            let timings = result.tokenTimings, !timings.isEmpty
        else { return result.text }
        do {
            guard let boosting = try await boosting(for: vocabulary, spotter: spotter) else {
                return result.text
            }
            let spotted = try await spotter.spotKeywordsWithLogProbs(
                audioSamples: samples, customVocabulary: boosting.context, minScore: nil)
            guard !spotted.logProbs.isEmpty else { return result.text }
            let tuning = ContextBiasingConstants.rescorerConfig(forVocabSize: boosting.context.terms.count)
            let output = boosting.rescorer.ctcTokenRescore(
                transcript: result.text,
                tokenTimings: timings,
                logProbs: spotted.logProbs,
                frameDuration: spotted.frameDuration,
                cbw: tuning.cbw,
                minSimilarity: tuning.minSimilarity
            )
            return output.wasModified ? output.text : result.text
        } catch {
            return result.text
        }
    }

    private func boosting(
        for terms: [String], spotter: CtcKeywordSpotter
    ) async throws -> (context: CustomVocabularyContext, rescorer: VocabularyRescorer)? {
        if let boosting, boosting.terms == terms { return (boosting.context, boosting.rescorer) }
        guard let tokenizer, let ctcDirectory else { return nil }
        let context = CustomVocabularyContext(
            terms: terms.compactMap { term in
                let ids = tokenizer.encode(term)
                return ids.isEmpty ? nil : CustomVocabularyTerm(text: term, ctcTokenIds: ids)
            })
        guard !context.terms.isEmpty else { return nil }
        let rescorer = try await VocabularyRescorer.create(
            spotter: spotter, vocabulary: context, ctcModelDirectory: ctcDirectory)
        boosting = (terms, context, rescorer)
        return (context, rescorer)
    }
}
