import Foundation

/// A chat as the phone holds it: how it stands, its rows, and each subagent's own rows.
struct ChatRows: Codable, Equatable {
    /// The head as the core last sent it, field by field, so a change of some fields keeps the rest.
    private(set) var headFields: [String: JSONValue]
    private(set) var head: ChatHead
    var messages: [ChatMessage]
    var subagentRows: [String: [ChatMessage]]
    /// Earlier rows start before this event; the core pages back to them.
    var olderBefore: Int?

    static let empty: ChatRows = {
        let head = ChatHead(
            connection: .connecting, running: false, turned: false, permissionMode: .unknown, commands: [],
            capabilities: ChatCapabilities(steering: false, editing: false), setup: .object([:]), permissions: [],
            tasks: [], runningSubagents: [], queued: [], triggers: [], earlierLost: false)
        guard case .object(let fields) = encodeJSON(head) else { fatalError("a chat head is a JSON object") }
        return ChatRows(headFields: fields, head: head, messages: [], subagentRows: [:], olderBefore: nil)
    }()

    init(
        headFields: [String: JSONValue], head: ChatHead, messages: [ChatMessage], subagentRows: [String: [ChatMessage]],
        olderBefore: Int?
    ) {
        self.headFields = headFields
        self.head = head
        self.messages = messages
        self.subagentRows = subagentRows
        self.olderBefore = olderBefore
    }

    init(view: ChatView) {
        var fields = Self.empty.headFields
        if case .object(let sent) = encodeJSON(view.head) {
            for (key, value) in sent { fields[key] = value }
        }
        self.init(
            headFields: fields, head: view.head, messages: view.rows, subagentRows: view.subagents ?? [:],
            olderBefore: view.olderBefore)
    }

    var running: Bool { head.running }
    /// Nil until the core sends the chat's head, or when it names a mode this build does not know.
    var permissionMode: PermissionMode? { head.permissionMode == .unknown ? nil : head.permissionMode }

    mutating func mergeHead(_ set: JSONValue) {
        guard case .object(let changed) = set else { return }
        for (key, value) in changed {
            if value == .null { headFields[key] = nil } else { headFields[key] = value }
        }
        if let read: ChatHead = decodeJSON(.object(headFields)) { head = read }
    }

    mutating func setHead(_ edit: (inout ChatHead) -> Void) {
        edit(&head)
        if case .object(let fields) = encodeJSON(head) { headFields = fields }
    }

    /// Rows from before the first one held go above it.
    mutating func addPage(_ page: ChatPage) {
        let held = Set(messages.map(\.id))
        messages = page.rows.filter { !held.contains($0.id) } + messages
        subagentRows = (page.subagents ?? [:]).merging(subagentRows) { _, kept in kept }
        olderBefore = page.olderBefore
    }
}

/// What a batch of the core's changes touched, so the transcript redraws only those rows.
struct Touched {
    var rows = Set<String>()
    /// The rows were replaced or cut, so every one is drawn again.
    var all = false
    var head = false

    mutating func formUnion(_ other: Touched) {
        rows.formUnion(other.rows)
        all = all || other.all
        head = head || other.head
    }
}

extension ChatRows {
    /// The chat with the core's changes made.
    mutating func apply(_ changes: [ChatChange], touched: inout Touched) {
        for change in changes { apply(change, touched: &touched) }
    }

    private mutating func apply(_ change: ChatChange, touched: inout Touched) {
        switch change {
        case .head(let head):
            mergeHead(head.set)
            touched.head = true
        case .row(let change):
            within(change.in, &touched, change.row.id) { rows in
                if let at = rows.lastIndex(where: { $0.id == change.row.id }) {
                    rows[at] = change.row
                } else {
                    rows.append(change.row)
                }
            }
        case .part(let change):
            within(change.in, &touched, change.row) { rows in
                guard let at = rows.lastIndex(where: { $0.id == change.row }) else { return }
                if let index = rows[at].parts.firstIndex(where: { $0.id == change.part.id }) {
                    rows[at].parts[index] = change.part
                } else {
                    rows[at].parts.append(change.part)
                }
            }
        case .text(let change):
            within(change.in, &touched, change.row) { rows in
                guard let at = rows.lastIndex(where: { $0.id == change.row }),
                    let index = rows[at].parts.firstIndex(where: { $0.id == change.part })
                else { return }
                rows[at].parts[index].append(change.text, change.md)
            }
        case .set(let change):
            within(change.in, &touched, change.row) { rows in
                guard let at = rows.lastIndex(where: { $0.id == change.row }),
                    case .object(var fields) = encodeJSON(rows[at]),
                    case .object(let set) = change.set
                else { return }
                for (key, value) in set {
                    if value == .null { fields[key] = nil } else { fields[key] = value }
                }
                if let row: ChatMessage = decodeJSON(.object(fields)) { rows[at] = row }
            }
        case .keep(let keep):
            let kept = Set(keep.rows)
            if let first = messages.firstIndex(where: { kept.contains($0.id) }) {
                messages = Array(messages[..<first]) + messages[first...].filter { kept.contains($0.id) }
            } else {
                messages = []
            }
            touched.all = true
        case .drop(let drop):
            if let at = messages.firstIndex(where: { $0.id == drop.from }) { messages.removeSubrange(at...) }
            touched.all = true
        case .view(let view):
            self = ChatRows(view: view.view)
            touched.all = true
            touched.head = true
        case .unknown:
            break
        }
    }

    private mutating func within(
        _ session: String?, _ touched: inout Touched, _ row: String, _ change: (inout [ChatMessage]) -> Void
    ) {
        if let session {
            change(&subagentRows[session, default: []])
            touched.head = true
        } else {
            change(&messages)
            touched.rows.insert(row)
        }
    }
}

extension ChatPart {
    var id: String {
        switch self {
        case .text(let part): part.id
        case .thought(let part): part.id
        case .content(let part): part.id
        case .tool(let part): part.id
        case .subagent(let part): part.id
        case .notice(let part): part.id
        case .agentNotice(let part): part.id
        case .account(let part): part.id
        case .unknown(let kind): kind
        }
    }

    /// Text written on to the end of a text or thought part, with its tree following while each change fits.
    mutating func append(_ text: String, _ change: MdChange?) {
        switch self {
        case .text(var part):
            part.text += text
            part.md = nextTree(part.md, change, part.text)
            self = .text(part)
        case .thought(var part):
            part.text += text
            part.md = nextTree(part.md, change, part.text)
            self = .thought(part)
        default:
            break
        }
    }
}

/// A part's tree follows the core's changes while each one fits the text it ends at; one that does not loses it
/// until the core sends a whole tree.
private func nextTree(_ tree: PartTree?, _ change: MdChange?, _ text: String) -> PartTree? {
    guard let change else { return tree }
    var whole = false
    if case .list(let first) = change.o.first, first.p.isEmpty, first.at == 0 { whole = true }
    if !whole && tree == nil { return nil }
    var blocks = whole ? [] : tree?.blocks ?? []
    for op in change.o { MdPatch.apply(op, to: &blocks) }
    return change.n == text.utf16.count ? PartTree(blocks: blocks, n: change.n) : nil
}

/// The core sends a head's and a row's changes field by field, so they are laid over the JSON of what is held and read
/// back; a change this build cannot read leaves what is held as it was.
func decodeJSON<T: Decodable>(_ value: JSONValue) -> T? {
    do {
        return try JSONDecoder().decode(T.self, from: JSONEncoder().encode(value))
    } catch {
        print("sikemux: could not read a chat change as \(T.self): \(error)")
        return nil
    }
}

func encodeJSON<T: Encodable>(_ value: T) -> JSONValue {
    do {
        return try JSONDecoder().decode(JSONValue.self, from: JSONEncoder().encode(value))
    } catch {
        print("sikemux: could not write \(T.self) as JSON: \(error)")
        return .null
    }
}

extension ChatRows {
    var permissions: [PermissionAsk] {
        get { head.permissions }
        set { setHead { $0.permissions = newValue } }
    }

    var queued: [QueuedMessage] { head.queued }
    var changingMode: Bool { head.changingMode == true }
    var draft: ChatDraft? { head.draft }
    var model: ConfigPicker? { head.model }
    var effort: ConfigPicker? { head.effort }
    var usage: ContextUsage? { head.usage }
    var commands: [ChatCommandInfo] { head.commands }
}
