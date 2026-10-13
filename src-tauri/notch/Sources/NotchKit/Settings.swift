import Foundation

extension NotchSettings {
    static func load(_ path: String) -> NotchSettings {
        guard let data = FileManager.default.contents(atPath: path),
              let settings = try? JSONDecoder().decode(NotchSettings.self, from: data)
        else { return NotchSettings() }
        return settings
    }
}

/// Calls back on the main queue whenever the settings file is written or replaced.
final class SettingsWatcher {
    private let path: String
    private var source: DispatchSourceFileSystemObject?
    private let changed: () -> Void

    init(path: String, changed: @escaping () -> Void) {
        self.path = path
        self.changed = changed
        watch()
    }

    private func watch() {
        let directory = (path as NSString).deletingLastPathComponent
        try? FileManager.default.createDirectory(atPath: directory, withIntermediateDirectories: true)
        let descriptor = open(directory, O_EVTONLY)
        guard descriptor >= 0 else { return }
        let source = DispatchSource.makeFileSystemObjectSource(fileDescriptor: descriptor, eventMask: [.write], queue: .main)
        source.setEventHandler { [weak self] in self?.changed() }
        source.setCancelHandler { Darwin.close(descriptor) }
        source.resume()
        self.source = source
    }
}
