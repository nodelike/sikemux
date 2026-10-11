import Foundation

/// The Xcode the simulator runs from. The selected one is used when it can run simulators, else the
/// one in /Applications, so a Mac left on the Command Line Tools still works.
enum Xcode {
    static let missing =
        "The iOS Simulator needs Xcode. Select Xcode with `sudo xcode-select -s /Applications/Xcode.app`, or install it from the App Store and open it once."

    /// Points DEVELOPER_DIR at a usable Xcode before the simulator libraries first read it.
    static func choose() throws {
        if let selected = selected(), canRunSimulators(selected) { return }
        guard let installed = installed().first(where: canRunSimulators) else {
            throw Failure(reason: "noXcode", message: missing)
        }
        setenv("DEVELOPER_DIR", installed, 1)
    }

    static func canRunSimulators(_ developerDir: String) -> Bool {
        FileManager.default.fileExists(atPath: "\(developerDir)/Platforms/iPhoneSimulator.platform")
    }

    private static func selected() -> String? {
        if let given = ProcessInfo.processInfo.environment["DEVELOPER_DIR"], !given.isEmpty { return given }
        let process = Process()
        let output = Pipe()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/xcode-select")
        process.arguments = ["--print-path"]
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        guard (try? process.run()) != nil else { return nil }
        let path = String(decoding: output.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        process.waitUntilExit()
        return process.terminationStatus == 0 && !path.isEmpty ? path : nil
    }

    /// /Applications/Xcode.app first, then any other Xcode there, newest first.
    static func installed(in applications: String = "/Applications") -> [String] {
        let names = (try? FileManager.default.contentsOfDirectory(atPath: applications)) ?? []
        let others = names.filter { $0.hasPrefix("Xcode") && $0.hasSuffix(".app") && $0 != "Xcode.app" }
            .map { "\(applications)/\($0)" }
            .sorted { version(of: $0).compare(version(of: $1), options: .numeric) == .orderedDescending }
        return (["\(applications)/Xcode.app"] + others).map { "\($0)/Contents/Developer" }
    }

    private static func version(of app: String) -> String {
        NSDictionary(contentsOfFile: "\(app)/Contents/Info.plist")?["CFBundleShortVersionString"] as? String ?? "0"
    }
}
