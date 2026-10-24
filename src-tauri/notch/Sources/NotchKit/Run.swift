import AppKit

// The island over the MacBook notch: the agents Sikemux's core runs, what
// they need from the person, and a composer to start another. Sikemux starts
// this helper; it reads the same view of the core that paired phones get.

public func runNotch(_ arguments: [String]) -> Never {
    guard let options = Options.parse(arguments) else {
        FileHandle.standardError.write(Data("""
        usage: sikemux-notch --socket PATH --protocol-version N --state-dir DIR
               [--core-binary PATH] [--app PATH] [--fonts DIR]
               [--dev [--app-pid PID]]

        """.utf8))
        exit(2)
    }
    guard Handover.claim(stateDir: options.stateDir, dev: options.dev) else { exit(0) }
    Debug.on = FileManager.default.fileExists(atPath: (options.stateDir as NSString).appendingPathComponent("notch.debug"))
    if options.dev, let pid = options.appPid { exitWith(pid) }
    let app = NSApplication.shared
    app.setActivationPolicy(.accessory)
    let delegate = AppDelegate(options: options)
    app.delegate = delegate
    app.run()
    exit(0)
}

/// Ends this helper when the dev app that started it ends.
private func exitWith(_ pid: pid_t) {
    guard kill(pid, 0) == 0 else { exit(0) }
    let source = DispatchSource.makeProcessSource(identifier: pid, eventMask: .exit, queue: .main)
    source.setEventHandler { exit(0) }
    source.resume()
    appWatch = source
}

private var appWatch: DispatchSourceProcess?

final class AppDelegate: NSObject, NSApplicationDelegate {
    private let store: NotchStore
    private var panels: Panels?

    init(options: Options) {
        Theme.registerFonts(in: options.fonts)
        store = NotchStore(options: options)
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        panels = Panels(store: store)
        store.connect()
    }
}
