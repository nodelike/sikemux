import AVFoundation
import Foundation

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
    private let engine = AVAudioEngine()
    private let lock = NSLock()
    private var samples: [Float] = []
    private var sampleRate: Double = 16_000

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
        lock.lock()
        samples.removeAll(keepingCapacity: true)
        sampleRate = format.sampleRate
        lock.unlock()
        input.installTap(onBus: 0, bufferSize: 2048, format: format) { [weak self] buffer, _ in
            self?.append(buffer)
        }
        engine.prepare()
        do {
            try engine.start()
        } catch {
            input.removeTap(onBus: 0)
            throw error
        }
    }

    func replay(_ audio: [Float], sampleRate: Double) {
        lock.lock()
        samples.removeAll(keepingCapacity: true)
        self.sampleRate = sampleRate
        lock.unlock()
        let step = Int(sampleRate / 10)
        Thread.detachNewThread { [weak self] in
            for start in stride(from: 0, to: audio.count, by: step) {
                guard let self else { return }
                self.lock.lock()
                self.samples.append(contentsOf: audio[start..<min(start + step, audio.count)])
                self.lock.unlock()
                Thread.sleep(forTimeInterval: 0.1)
            }
        }
    }

    func snapshot() -> (samples: [Float], sampleRate: Double) {
        lock.lock()
        defer { lock.unlock() }
        return (samples, sampleRate)
    }

    func stop() -> (samples: [Float], sampleRate: Double) {
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
        lock.lock()
        defer { lock.unlock() }
        let captured = samples
        samples = []
        return (captured, sampleRate)
    }

    private func append(_ buffer: AVAudioPCMBuffer) {
        guard let channels = buffer.floatChannelData else { return }
        let frames = Int(buffer.frameLength)
        let channelCount = Int(buffer.format.channelCount)
        var mono = [Float](repeating: 0, count: frames)
        for channel in 0..<channelCount {
            let data = channels[channel]
            for frame in 0..<frames { mono[frame] += data[frame] }
        }
        if channelCount > 1 {
            let scale = 1 / Float(channelCount)
            for frame in 0..<frames { mono[frame] *= scale }
        }
        lock.lock()
        samples.append(contentsOf: mono)
        lock.unlock()
    }
}
