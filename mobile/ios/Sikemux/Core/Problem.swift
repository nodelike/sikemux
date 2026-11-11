import Foundation
import SikemuxCore

/// What went wrong, in words for the person: what the host said when it refused, else what the error says.
func problem(_ error: Error) -> String {
    if let error = error as? MobileError {
        switch error {
        case .Refused(let message, _), .Connection(let message), .Invalid(let message): return message
        case .Unpaired: return "This host no longer knows this phone. Forget it, then connect again."
        case .Outdated(let macIsOlder):
            return macIsOlder
                ? "Update Sikemux on this host to reach it from this app." : "Update this app to reach this host."
        }
    }
    if let error = error as? LocalizedError, let description = error.errorDescription { return description }
    return error.localizedDescription
}

extension MobileError {
    /// What kind of refusal the host met, when it names one this build knows.
    var refusal: ErrorKind? {
        if case .Refused(_, let kind) = self { return kind.flatMap(ErrorKind.init) }
        return nil
    }
}
