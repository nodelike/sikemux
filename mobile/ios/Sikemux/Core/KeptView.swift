import Foundation

/// The last `DeviceView` heard from a host, so returning or relaunching draws the list at once while reconnecting.
final class KeptView: @unchecked Sendable {
    static let shared = KeptView()

    private static let maxBytes = 4 * 1024 * 1024
    private static let saveDelay: TimeInterval = 2
    private static let folder = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
        .appendingPathComponent("views")

    private let queue = DispatchQueue(label: "sikemux.kept-views", qos: .utility)
    private var pending: [String: (view: DeviceView, work: DispatchWorkItem)] = [:]

    private init() {}

    private static func file(for core: String) -> URL {
        let name = core.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? core
        return folder.appendingPathComponent("\(name).json")
    }

    func load(_ core: String) -> DeviceView? {
        let file = Self.file(for: core)
        guard FileManager.default.fileExists(atPath: file.path) else { return nil }
        guard let data = try? Data(contentsOf: file) else { return nil }
        return try? JSONDecoder().decode(DeviceView.self, from: data)
    }

    func save(_ core: String, view: DeviceView) {
        queue.async {
            if let existing = self.pending[core] {
                self.pending[core] = (view, existing.work)
                return
            }
            let work = DispatchWorkItem {
                guard let entry = self.pending.removeValue(forKey: core) else { return }
                Self.writeNow(core, view: entry.view)
            }
            self.pending[core] = (view, work)
            self.queue.asyncAfter(deadline: .now() + Self.saveDelay, execute: work)
        }
    }

    func flush() {
        queue.async {
            let items = self.pending
            self.pending.removeAll()
            for (core, entry) in items {
                entry.work.cancel()
                Self.writeNow(core, view: entry.view)
            }
        }
    }

    func forget(_ core: String) {
        queue.async {
            if let existing = self.pending.removeValue(forKey: core) {
                existing.work.cancel()
            }
            let file = Self.file(for: core)
            try? FileManager.default.removeItem(at: file)
        }
    }

    private static func writeNow(_ core: String, view: DeviceView) {
        guard let data = try? JSONEncoder().encode(view), data.count <= maxBytes else { return }
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        try? data.write(to: file(for: core), options: .atomic)
    }
}
