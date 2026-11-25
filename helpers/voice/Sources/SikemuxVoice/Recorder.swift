import AVFoundation
import AudioRing
import Foundation
import os

enum RecorderError: LocalizedError {
    case microphoneDenied
    case noInput

    var errorDescription: String? {
        switch self {
        case .microphoneDenied:
            return "Sikemux is not allowed to use the microphone. Allow it in System Settings → Privacy & Security → Microphone."
        case .noInput:
            return "No microphone is available."
        }
    }
}

final class Recorder {
    static let targetSampleRate: Double = 16_000
    static let maxDurationSeconds: Double = 300
    static let maxSamples = Int(targetSampleRate * maxDurationSeconds)

    private static let ringCapacity = 131_072
    private static let drainChunkFrames = 4_096

    private struct Storage {
        var samples: [Float] = []
        var stopped = false
    }

    private let engine = AVAudioEngine()
    private let drainQueue = DispatchQueue(label: "sikemux-voice.audio", qos: .userInitiated)
    private let storage = OSAllocatedUnfairLock(initialState: Storage())
    private let ring: OpaquePointer
    private var drainSignal: DispatchSourceUserDataAdd?
    private var converter: AVAudioConverter?
    private var inputChunk: AVAudioPCMBuffer?
    private var outputChunk: AVAudioPCMBuffer?
    private var engineRunning = false

    init() {
        guard let ring = audio_ring_create(Self.ringCapacity) else {
            fatalError("Failed to allocate audio ring buffer")
        }
        self.ring = ring
    }

    deinit {
        if engineRunning {
            engine.inputNode.removeTap(onBus: 0)
            engine.stop()
        }
        drainSignal?.cancel()
        audio_ring_destroy(ring)
    }

    static func ensurePermission() async throws {
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized:
            return
        case .notDetermined:
            if await AVCaptureDevice.requestAccess(for: .audio) { return }
            throw RecorderError.microphoneDenied
        default:
            throw RecorderError.microphoneDenied
        }
    }

    func start() throws {
        let input = engine.inputNode
        let format = input.outputFormat(forBus: 0)
        guard format.sampleRate > 0, format.channelCount > 0 else { throw RecorderError.noInput }
        guard
            let monoInputFormat = AVAudioFormat(
                commonFormat: .pcmFormatFloat32,
                sampleRate: format.sampleRate,
                channels: 1,
                interleaved: false
            ),
            let targetFormat = AVAudioFormat(
                commonFormat: .pcmFormatFloat32,
                sampleRate: Self.targetSampleRate,
                channels: 1,
                interleaved: false
            ),
            let inBuffer = AVAudioPCMBuffer(
                pcmFormat: monoInputFormat,
                frameCapacity: AVAudioFrameCount(Self.drainChunkFrames)
            )
        else {
            throw RecorderError.noInput
        }

        let audioConverter: AVAudioConverter?
        let outBuffer: AVAudioPCMBuffer?
        if format.sampleRate == Self.targetSampleRate {
            audioConverter = nil
            outBuffer = nil
        } else {
            guard let made = AVAudioConverter(from: monoInputFormat, to: targetFormat) else {
                throw RecorderError.noInput
            }
            made.sampleRateConverterAlgorithm = AVSampleRateConverterAlgorithm_Mastering
            made.sampleRateConverterQuality = AVAudioQuality.max.rawValue
            let outCapacity = AVAudioFrameCount(
                (Double(Self.drainChunkFrames) * Self.targetSampleRate / format.sampleRate).rounded(.up)
            ) + 64
            guard let madeOut = AVAudioPCMBuffer(pcmFormat: targetFormat, frameCapacity: outCapacity) else {
                throw RecorderError.noInput
            }
            audioConverter = made
            outBuffer = madeOut
        }

        audio_ring_reset(ring)
        storage.withLock { state in
            state.samples.removeAll(keepingCapacity: true)
            state.stopped = false
        }
        inputChunk = inBuffer
        outputChunk = outBuffer
        converter = audioConverter

        let signal = DispatchSource.makeUserDataAddSource(queue: drainQueue)
        signal.setEventHandler { [weak self] in
            self?.drainAvailable()
        }
        drainSignal = signal
        signal.resume()

        let ring = self.ring
        input.installTap(onBus: 0, bufferSize: 2048, format: format) { buffer, _ in
            guard let channels = buffer.floatChannelData else { return }
            let frames = Int(buffer.frameLength)
            let channelCount = Int(buffer.format.channelCount)
            if audio_ring_write_mono(ring, channels, channelCount, frames) > 0 {
                signal.add(data: 1)
            }
        }
        engine.prepare()
        do {
            try engine.start()
            engineRunning = true
        } catch {
            input.removeTap(onBus: 0)
            signal.cancel()
            drainQueue.sync {}
            drainSignal = nil
            throw error
        }
    }

    func replay(_ audio16k: [Float]) {
        storage.withLock { state in
            state.samples.removeAll(keepingCapacity: true)
            state.stopped = false
        }
        let step = Int(Self.targetSampleRate / 10)
        Thread.detachNewThread { [weak self] in
            for start in stride(from: 0, to: audio16k.count, by: step) {
                guard let self else { return }
                let end = min(start + step, audio16k.count)
                let keepGoing = self.storage.withLock { state -> Bool in
                    guard !state.stopped else { return false }
                    let room = Self.maxSamples - state.samples.count
                    guard room > 0 else { return false }
                    let limit = min(end, start + room)
                    state.samples.append(contentsOf: audio16k[start..<limit])
                    return true
                }
                if !keepGoing { return }
                Thread.sleep(forTimeInterval: 0.1)
            }
        }
    }

    func sampleCount() -> Int {
        storage.withLock { $0.samples.count }
    }

    func slice(from start: Int, maxCount: Int) -> (samples: [Float], totalCount: Int) {
        storage.withLock { state in
            let total = state.samples.count
            guard start < total, maxCount > 0 else { return ([], total) }
            let end = min(total, start + maxCount)
            return (Array(state.samples[start..<end]), total)
        }
    }

    func stop() -> [Float] {
        if engineRunning {
            engine.inputNode.removeTap(onBus: 0)
            engine.stop()
            engineRunning = false
            drainSignal?.cancel()
            drainQueue.sync {
                self.drainAvailable()
                self.flushConverter()
                self.converter = nil
                self.inputChunk = nil
                self.outputChunk = nil
            }
            drainSignal = nil
        }
        return storage.withLock { state in
            state.stopped = true
            let captured = state.samples
            state.samples = []
            return captured
        }
    }

    private func drainAvailable() {
        guard let inputChunk, let inputChannel = inputChunk.floatChannelData?[0] else { return }
        while true {
            let read = audio_ring_read(ring, inputChannel, Self.drainChunkFrames)
            if read == 0 { break }
            inputChunk.frameLength = AVAudioFrameCount(read)
            if let converter, let outputChunk {
                var provided = false
                let inputBlock: AVAudioConverterInputBlock = { _, status in
                    if !provided {
                        provided = true
                        status.pointee = .haveData
                        return inputChunk
                    }
                    status.pointee = .noDataNow
                    return nil
                }
                while true {
                    outputChunk.frameLength = outputChunk.frameCapacity
                    var error: NSError?
                    let status = converter.convert(to: outputChunk, error: &error, withInputFrom: inputBlock)
                    if status == .error { break }
                    if outputChunk.frameLength > 0, let outChannel = outputChunk.floatChannelData?[0] {
                        append16k(UnsafeBufferPointer(start: outChannel, count: Int(outputChunk.frameLength)))
                    }
                    if status != .haveData { break }
                }
            } else {
                append16k(UnsafeBufferPointer(start: inputChannel, count: read))
            }
        }
    }

    private func flushConverter() {
        guard let converter, let outputChunk else { return }
        let endBlock: AVAudioConverterInputBlock = { _, status in
            status.pointee = .endOfStream
            return nil
        }
        while true {
            outputChunk.frameLength = outputChunk.frameCapacity
            var error: NSError?
            let status = converter.convert(to: outputChunk, error: &error, withInputFrom: endBlock)
            if status == .error { break }
            if outputChunk.frameLength > 0, let outChannel = outputChunk.floatChannelData?[0] {
                append16k(UnsafeBufferPointer(start: outChannel, count: Int(outputChunk.frameLength)))
            }
            if status == .endOfStream || outputChunk.frameLength == 0 { break }
        }
    }

    private func append16k(_ samples: UnsafeBufferPointer<Float>) {
        guard !samples.isEmpty else { return }
        storage.withLockUnchecked { state in
            let room = Self.maxSamples - state.samples.count
            guard room > 0 else { return }
            let count = min(room, samples.count)
            state.samples.append(contentsOf: samples.prefix(count))
        }
    }
}
