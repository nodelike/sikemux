import Foundation

/// What the app tells the helper when it starts it. The core binary is the
/// app's own, whose `start` runs the core exactly as the app would.
struct Options {
    var socket = ""
    var protocolVersion = 0
    var coreBinary: String?
    var dev = false
    /// A dev build's app: its island goes away with it, so a stable build's comes back.
    var appPid: pid_t?
    /// The app bundle, or the app's executable in a dev build, to open when its window is not.
    var app: String?
    var fonts: String?
    var settings = ""
    var stateDir = ""

    static let version = "sikemux-notch 1"

    static func parse(_ arguments: [String]) -> Options? {
        var options = Options()
        var rest = arguments.dropFirst().makeIterator()
        while let flag = rest.next() {
            if flag == "--version" {
                print(version)
                exit(0)
            }
            if flag == "--dev" {
                options.dev = true
                continue
            }
            guard let value = rest.next() else { return nil }
            switch flag {
            case "--socket": options.socket = value
            case "--protocol-version": options.protocolVersion = Int(value) ?? 0
            case "--core-binary": options.coreBinary = value
            case "--app": options.app = value
            case "--app-pid": options.appPid = pid_t(value)
            case "--fonts": options.fonts = value
            case "--settings": options.settings = value
            case "--state-dir": options.stateDir = value
            default: return nil
            }
        }
        guard !options.socket.isEmpty, options.protocolVersion > 0, !options.settings.isEmpty, !options.stateDir.isEmpty
        else { return nil }
        return options
    }
}
