import Foundation
import Observation

/// Every screen the app opens, named as the Android app's routes are. Links and notifications name them by path:
/// `/device/<core>?tab=terminals`, `/device/<core>/chat/<agent>`, `/device/<core>/new?project=…`.
enum Route: Hashable {
    enum DeviceTab: String {
        case agents, terminals, recent, calls
    }

    case device(core: String, tab: DeviceTab? = nil)
    case chat(core: String, agent: String)
    case newChat(core: String, project: String? = nil, launcher: String? = nil)
    case pairing(core: String)
    case hostSignIn
    case deleteAccount
    case share(id: String, key: String?)
    case signIn

    /// The route a link names, from its path and query, without the scheme.
    init?(url: URL) {
        let parts = URLComponents(url: url, resolvingAgainstBaseURL: false)
        // `sikemux://device/x` puts the first word in the host.
        let words = ([url.host].compactMap { $0 } + url.pathComponents.filter { $0 != "/" })
        var query: [String: String] = [:]
        for item in parts?.queryItems ?? [] { query[item.name] = item.value }
        switch words.first {
        case "device":
            guard words.count >= 2 else { return nil }
            let core = words[1]
            switch words.count > 2 ? words[2] : nil {
            case nil: self = .device(core: core, tab: query["tab"].flatMap(DeviceTab.init))
            case "chat" where words.count > 3:
                self = .chat(core: core, agent: words[3])
            case "new": self = .newChat(core: core, project: query["project"], launcher: query["launcher"])
            case "pairing": self = .pairing(core: core)
            default: return nil
            }
        case "host-sign-in": self = .hostSignIn
        case "delete-account": self = .deleteAccount
        case "share":
            guard words.count >= 2 else { return nil }
            // A shared chat's key rides in the link's fragment, which never reaches a server.
            self = .share(id: words[1], key: query["key"] ?? parts?.fragment)
        case "sign-in": self = .signIn
        default: return nil
        }
    }

    /// The screens a link to this one opens over, so back goes where it would have.
    var under: [Route] {
        switch self {
        case .chat(let core, _), .newChat(let core, _, _), .pairing(let core): [.device(core: core)]
        default: []
        }
    }
}

/// The stack of screens over the devices list.
@MainActor @Observable
final class Router {
    static let shared = Router()

    var path: [Route] = []
    /// Links wait here while the app is locked, so one cannot open a chat or start anything behind the lock.
    var holding = false
    private var held: Route?

    func push(_ route: Route) {
        path.append(route)
    }

    /// Takes the top screen's place, as starting a chat replaces the new-chat screen with the chat.
    func replace(_ route: Route) {
        if path.isEmpty { path = [route] } else { path[path.count - 1] = route }
    }

    func back() {
        if !path.isEmpty { path.removeLast() }
    }

    func root() {
        path = []
    }

    /// Opens what a link names over the devices list, unless the app is locked; it opens once it is not.
    func open(_ url: URL) {
        guard let route = Route(url: url) else { return }
        open(route)
    }

    func open(_ route: Route) {
        if holding {
            held = route
            return
        }
        path = route.under + [route]
    }

    func release() {
        holding = false
        if let held {
            self.held = nil
            open(held)
        }
    }
}
