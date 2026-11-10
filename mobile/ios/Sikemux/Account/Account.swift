import Foundation
import Observation

/// Who this phone is signed in as. The sign-in itself, the session's tokens and leaving the account live here.
@MainActor @Observable
final class Account {
    enum State: Equatable {
        case loading
        case signedOut
        case signedIn(userId: String)
    }

    static let shared = Account()

    private(set) var state: State = .loading

    var isSignedIn: Bool {
        if case .signedIn = state { return true }
        return false
    }

    func load() async {
        state = .signedOut
    }
}
