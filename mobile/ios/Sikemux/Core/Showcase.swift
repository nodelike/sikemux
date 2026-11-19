#if DEBUG
    import Foundation
    import SikemuxCore

    /// A Dev build launched with `-SikemuxShowcase <file>` runs signed in to nobody, paired with a pretend Mac that
    /// answers from the file `mobile/showcase/export.mjs` writes, for pictures of its screens without an account.
    /// `-SikemuxShowcaseOpen <url>` then opens one of its links.
    struct Showcase: Sendable {
        let view: String
        let host: String
        let calls: [String: String]
        let chats: [String: String]
        let details: [String: [String: String]]
        let paired: [PairedDevice]

        static let current: Showcase? = argument("-SikemuxShowcase").flatMap(read)

        static var opening: URL? { argument("-SikemuxShowcaseOpen").flatMap(URL.init(string:)) }

        static let user = Clerk.User(
            id: "showcase", firstName: "Edon", lastName: nil, imageUrl: nil, hasImage: false,
            primaryEmailAddressId: nil, emailAddresses: [], externalAccounts: nil, passwordEnabled: nil)

        fileprivate static func argument(_ name: String) -> String? {
            let arguments = ProcessInfo.processInfo.arguments
            guard let at = arguments.firstIndex(of: name), at + 1 < arguments.count else { return nil }
            return arguments[at + 1]
        }

        private static func read(_ path: String) -> Showcase? {
            guard let data = FileManager.default.contents(atPath: path),
                let world = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
            else {
                print("sikemux: the showcase file \(path) could not be read")
                return nil
            }
            func text(_ value: Any?) -> String {
                guard let value,
                    let data = try? JSONSerialization.data(withJSONObject: value, options: .fragmentsAllowed)
                else { return "null" }
                return String(decoding: data, as: UTF8.self)
            }
            let texts = { (value: Any?) in ((value as? [String: Any]) ?? [:]).mapValues(text) }
            let paired = (try? JSONDecoder().decode([PairedDevice].self, from: Data(text(world["paired"]).utf8))) ?? []
            return Showcase(
                view: text(world["view"]), host: text(world["host"]), calls: texts(world["calls"]),
                chats: texts(world["chats"]),
                details: ((world["details"] as? [String: Any]) ?? [:]).mapValues(texts), paired: paired)
        }
    }

    /// With `-SikemuxShowcase` and `-SikemuxShowcaseAccount <file>`, the accounts server's answers come from a file that maps "POST /v1/…" to a
    /// list of `{"status": 409, "json": …}`, taken in turn; the last one repeats. A route not in it goes to the server.
    @MainActor
    enum ShowcaseAccount {
        private static var answers: [String: [[String: Any]]] = {
            guard Showcase.current != nil, let path = Showcase.argument("-SikemuxShowcaseAccount"),
                let data = FileManager.default.contents(atPath: path),
                let file = try? JSONSerialization.jsonObject(with: data) as? [String: [[String: Any]]]
            else { return [:] }
            return file
        }()

        static func answer(_ method: String, _ path: String) -> (status: Int, body: Data)? {
            let route = "\(method) \(path)"
            guard let list = answers[route], let next = list.first else { return nil }
            if list.count > 1 { answers[route] = Array(list.dropFirst()) }
            let body = next["json"].flatMap { try? JSONSerialization.data(withJSONObject: $0) } ?? Data("{}".utf8)
            return (next["status"] as? Int ?? 200, body)
        }
    }

    /// The pretend Mac's connection: it sends the view once and answers every request from the showcase file.
    final class ShowcaseConnection: Connection, @unchecked Sendable {
        private let world: Showcase
        private var open = true

        init(_ world: Showcase, listener: CoreListener) {
            self.world = world
            super.init(noHandle: NoHandle())
            let view = world.view
            Task.detached { try? listener.events(events: [.view(viewJson: view)]) }
        }

        required init(unsafeFromHandle handle: UInt64) { fatalError("a showcase connection has no Rust side") }

        private func unknown(_ what: String) -> MobileError {
            .Refused(message: "the showcase Mac does not answer \(what)", kind: nil)
        }

        override func call(method: String, paramsJson: String) async throws -> String {
            if method == "chatToolDetail" {
                let params = (try? JSONSerialization.jsonObject(with: Data(paramsJson.utf8))) as? [String: String]
                return world.details[params?["agentId"] ?? ""]?[params?["toolCallId"] ?? ""] ?? "{}"
            }
            if let answer = world.calls[method] { return answer }
            throw unknown(method)
        }
        override func attachChat(agentId: String, since: ChatMark?) async throws -> ChatAttachment {
            guard let chat = world.chats[agentId] else { return .missing }
            return .live(viewJson: chat, mark: ChatMark(feed: agentId, seq: 0))
        }
        override func host() async throws -> String { world.host }
        override func isOpen() -> Bool { open }
        override func close() { open = false }
        override func answerPermission(agentId: String, requestId: String, optionId: String?) async throws {}
        override func attachFile(agentId: String, name: String, mime: String, bytes: Data) async throws -> String {
            "/Users/demo/Library/Caches/com.nodelike.sikemux/pasted/\(name)"
        }
        override func callToFile(method: String, paramsJson: String, field: String, path: String) async throws
            -> String
        { throw unknown(method) }
        override func cancel(agentId: String) async throws {}
        override func chatHistory(agentId: String, feed: String, before: UInt64, turns: UInt32) async throws -> String {
            throw unknown("chat history")
        }
        override func clearNotifications() async throws {}
        override func detachChat(agentId: String) async throws {}
        override func prompt(agentId: String, messageId: String?, text: String, paths: [String]) async throws {}
        override func resumeChat(recent: String) async throws -> String { throw unknown("a recent chat") }
        override func saveBackdrop(dir: String, id: String) async throws -> String? { nil }
        override func setChatConfig(agentId: String, configId: String, value: String) async throws -> String { value }
        override func setForeground(foreground: Bool) async throws {}
        override func setNotifications(keyId: UInt32, key: Data, prefsJson: String) async throws {}
        override func setPermissionMode(agentId: String, mode: String) async throws {}
        override func startChat(launcher: String, project: String, permissionMode: String?) async throws -> String {
            throw unknown("a new chat")
        }
        override func steer(agentId: String, text: String, paths: [String]) async throws -> Bool { true }
        override func stopChat(agentId: String) async throws {}
        override func stopTask(agentId: String, taskId: String) async throws {}
        override func unpair() async throws {}
        override func wakeChat(agentId: String) async throws {}
    }
#endif
