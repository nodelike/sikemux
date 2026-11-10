import Foundation
import SikemuxCore
import SwiftUI

/// What the composer holds for the chat's next message, saved as the chat's own draft on the host.
@MainActor
final class ComposerDraft {
    var text = ""

    init(core: String, agentId: String, held: @escaping () -> ChatDraft?, live: @escaping () -> Connection?, emit: @escaping () -> Void) {}

    /// Takes in the host's draft, unless this phone holds writing the host has not saved yet.
    func takeIn() {}

    func save() async {}

    /// Writes down what is not saved on the host yet, so it outlives the app quitting.
    func keep() {}
}

/// A message this phone sent that the host does not show yet.
struct Unsent: Equatable {
    enum State { case sending, failed }
    var state: State
    var problem: String?
}

/// A file picked for a message, and where the host keeps it once it is there.
struct Attachment: Equatable, Identifiable {
    enum Kind { case image, file }
    var id: String
    var kind: Kind
    var name: String
    var mime: String
    var uri: URL
    var size: Int
    var path: String?
}

/// What this phone sent, until the host shows it.
@MainActor
final class Outbox {
    private(set) var unsent: [String: Unsent] = [:]
    private(set) var sentFiles: [String: Attachment] = [:]

    init(agentId: String, connection: @escaping () -> Connection?, chat: @escaping () -> ChatRows, emit: @escaping () -> Void) {}

    /// The chat with what this phone sent and the host does not show yet after it.
    func over(_ chat: ChatRows) -> ChatRows { chat }
    func dropTaken() {}
    func settle(turnEnded: Bool) {}
    func failWaiting(_ problem: String) {}
    func pushWaiting() {}
    func retry(_ messageId: String) {}
}

/// The bottom of a chat: the composer and the docks over it.
struct ChatBottom: View {
    let session: ChatSession
    let context: ChatBottomContext

    var body: some View {
        EmptyView()
    }
}

/// The questions other agents asked this chat, at the transcript's end.
struct ChatQuestionsFooter: View {
    let core: String
    let agentId: String
    let session: ChatSession

    var body: some View {
        EmptyView()
    }
}

/// Asks the composer's input to take the keyboard.
@MainActor
final class ComposerFocus: ObservableObject {
    func request() {}
}

/// The files a sent message carried.
struct SentAttachments: View {
    let paths: [String]
    var body: some View { EmptyView() }
}

/// The links and text a sent message carried, as chips.
struct ContextChips: View {
    let items: [SentContext]
    var body: some View { EmptyView() }
}

/// A question another agent asked, with the answer it got.
struct AnsweredQuestion: View {
    let own: Bool
    let asker: String
    let cli: String
    let question: String
    let answer: String
    let secret: Bool
    var body: some View { EmptyView() }
}

/// The working mark: a small spinner.
struct Working: View {
    var body: some View { ProgressView().controlSize(.mini) }
}
