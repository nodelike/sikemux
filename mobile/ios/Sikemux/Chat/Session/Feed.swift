import Foundation
import QuartzCore
import SikemuxCore

/// The chat as the host folds it, and where its changes got to.
@MainActor
final class Feed {
    enum Earlier { case idle, loading, failed }

    /// Turns of earlier rows asked for at a time.
    private static let pageTurns: UInt32 = 20
    /// Finished calls whose change and output are kept once read, so a row opened again shows them at once.
    private static let keptDetails = 40

    var chat = ChatRows.empty
    /// The last change the chat holds; a reconnect asks only for what comes after it.
    var mark: ChatMark?
    var earlier = Earlier.idle
    /// What changed since the transcript last drew.
    var touched = Touched()

    /// The chat changed from `before`.
    var applied: (ChatRows) -> Void = { _ in }
    var emit: () -> Void = {}
    /// Some changes never arrived, so the chat must be attached again from what was taken.
    var fellBehind: () -> Void = {}

    /// The last change taken, applied or not yet.
    private var taken: UInt64?
    private var pending: [ChatChange] = []
    private var frame: FrameOnce?
    private var paging: Task<Bool, Never>?
    /// Counts cuts, so a page that comes back after the chat was taken up again is dropped.
    private var generation = 0
    private var details: [String: Task<ToolDetail, Error>] = [:]
    private var detailOrder: [String] = []
    private var keptChat: ChatRows?
    private let store: KeptChat

    init(store: KeptChat) {
        self.store = store
    }

    /// Draws the chat as this phone last kept it; the first attach asks only for what came after.
    func restore() -> Bool {
        guard let kept = store.load() else { return false }
        chat = kept.chat
        keptChat = kept.chat
        mark = ChatMark(feed: kept.feed, seq: kept.seq)
        taken = kept.seq
        touched.all = true
        return true
    }

    /// Writes the chat down as drawn, unless it has not changed since it last was.
    func keep() {
        guard let mark, chat != keptChat else { return }
        keptChat = chat
        store.save(KeptChat.Kept(chat: chat, feed: mark.feed, seq: mark.seq))
    }

    /// Forgets where the chat got to, so the next attach reads it whole.
    func forget() {
        mark = nil
        taken = nil
    }

    /// Lets go of what the last attach had under way.
    func cut() {
        generation += 1
        if earlier == .loading { earlier = .idle }
        paging = nil
    }

    func apply(_ change: (inout ChatRows, inout Touched) -> Void) {
        let before = chat
        change(&chat, &touched)
        applied(before)
    }

    /// Starts again from the host's whole view, as of `mark`.
    func open(at view: ChatView, _ mark: ChatMark) {
        pending = []
        apply { chat, touched in
            chat = ChatRows(view: view)
            touched.all = true
            touched.head = true
        }
        attached(at: mark)
    }

    /// Catches up with what the host sent since the mark this phone held, up to `mark`.
    func resume(at changes: [ChatChange], _ mark: ChatMark) {
        apply { chat, touched in chat.apply(changes, touched: &touched) }
        attached(at: mark)
    }

    private func attached(at mark: ChatMark) {
        self.mark = mark
        taken = mark.seq
    }

    func take(_ deliveries: [ChatDelivery]) {
        for delivery in deliveries {
            if let taken, delivery.seq <= taken { continue }
            guard let changes = try? JSONDecoder().decode([ChatChange].self, from: Data(delivery.changesJson.utf8)) else {
                print("sikemux: could not read a chat change")
                continue
            }
            if let taken, delivery.seq != taken + 1, !Self.follows(changes, taken) {
                fellBehind()
                return
            }
            pending.append(contentsOf: changes)
            taken = delivery.seq
        }
        // A stream of tokens is drawn once a frame, not once a change.
        if frame == nil { frame = FrameOnce { [weak self] in self?.applyPending() } }
    }

    /// Whether a delivery that catches this phone up after it fell behind starts from `taken`.
    private static func follows(_ changes: [ChatChange], _ taken: UInt64) -> Bool {
        if case .head(let head) = changes.first, let follows = head.follows { return UInt64(follows) == taken }
        return false
    }

    /// Applies what was taken, and only then moves the mark past it.
    func applyPending() {
        frame?.cancel()
        frame = nil
        let advanced = mark != nil && taken != nil && taken != mark?.seq
        guard !pending.isEmpty || advanced else { return }
        let batch = pending
        pending = []
        if !batch.isEmpty { apply { chat, touched in chat.apply(batch, touched: &touched) } }
        if let taken, mark != nil { mark?.seq = taken }
        emit()
    }

    /// Answers whether earlier rows came; a fetch already under way is waited on rather than asked again.
    func pageBack(_ connection: Connection?, _ agentId: String) async -> Bool {
        if let paging { return await paging.value }
        guard let before = chat.olderBefore, let feed = mark?.feed, let connection else { return false }
        earlier = .loading
        emit()
        let generation = generation
        let paging = Task { @MainActor [weak self] () -> Bool in
            do {
                let json = try await connection.chatHistory(agentId: agentId, feed: feed, before: UInt64(before), turns: Self.pageTurns)
                let page = try await Task.detached { try JSONDecoder().decode(ChatPage.self, from: Data(json.utf8)) }.value
                guard let self, generation == self.generation else { return false }
                chat.addPage(page)
                touched.all = true
                earlier = .idle
                return true
            } catch {
                guard let self, generation == self.generation else { return false }
                if case MobileError.Refused = error {
                    chat.olderBefore = nil
                    earlier = .idle
                } else {
                    earlier = .failed
                }
                return false
            }
        }
        self.paging = paging
        let came = await paging.value
        if self.paging == paging { self.paging = nil }
        emit()
        return came
    }

    /// A call's whole change and what it printed, which rows carry only counts of. `done` calls are read once.
    func toolDetail(_ connection: Connection?, _ agentId: String, _ toolCallId: String, done: Bool) async throws -> ToolDetail {
        if let kept = details[toolCallId] { return try await kept.value }
        let reading = Task { @MainActor () throws -> ToolDetail in
            guard let connection, connection.isOpen() else { throw ChatProblem("the host is not connected") }
            return try await connection.call(PhoneCall.ChatToolDetail.self, ToolDetailParams(agentId: agentId, toolCallId: toolCallId))
        }
        guard done else { return try await reading.value }
        details[toolCallId] = reading
        detailOrder.append(toolCallId)
        if detailOrder.count > Self.keptDetails { details[detailOrder.removeFirst()] = nil }
        do {
            return try await reading.value
        } catch {
            details[toolCallId] = nil
            throw error
        }
    }
}

/// A problem the phone itself names, in the words shown to the person.
struct ChatProblem: LocalizedError {
    let errorDescription: String?
    init(_ text: String) { errorDescription = text }
}

/// Runs once, just before the next frame is drawn.
@MainActor
final class FrameOnce {
    private var link: CADisplayLink?
    private let run: () -> Void

    init(_ run: @escaping () -> Void) {
        self.run = run
        let link = CADisplayLink(target: Target(self), selector: #selector(Target.fire))
        link.add(to: .main, forMode: .common)
        self.link = link
    }

    func cancel() {
        link?.invalidate()
        link = nil
    }

    fileprivate func fire() {
        guard link != nil else { return }
        cancel()
        run()
    }

    private final class Target: NSObject {
        weak var owner: FrameOnce?
        init(_ owner: FrameOnce) { self.owner = owner }
        @objc func fire() { MainActor.assumeIsolated { owner?.fire() } }
    }
}
