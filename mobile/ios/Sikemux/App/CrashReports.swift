import CryptoKit
import Foundation
import Observation
import Sentry

/// Sends crashes and errors to Sentry while the person allows it. Only builds the release workflow made carry the
/// address to send to, so a dev build never sends.
@MainActor
@Observable
final class CrashReports {
    static let shared = CrashReports()
    private static let allowedKey = "sikemux.crashReports"
    private static let dsn = (Bundle.main.object(forInfoDictionaryKey: "SikemuxSentryDSN") as? String) ?? ""

    var on: Bool {
        didSet {
            UserDefaults.standard.set(on, forKey: Self.allowedKey)
            if on { start() } else { SentrySDK.close() }
        }
    }

    @ObservationIgnored private var account: String?

    private init() {
        on = UserDefaults.standard.object(forKey: Self.allowedKey) as? Bool ?? true
    }

    func start() {
        guard on, !Self.dsn.isEmpty, !SentrySDK.isEnabled else { return }
        let version = Versions.fromBuild(Config.build) ?? Config.version
        SentrySDK.start { options in
            options.dsn = Self.dsn
            options.releaseName = "sikemux-mobile@\(version)+\(Config.build)"
            options.environment = "production"
            options.sendDefaultPii = false
            options.enableNetworkBreadcrumbs = false
            options.enableCaptureFailedRequests = false
            options.enableAutoPerformanceTracing = false
        }
        SentrySDK.configureScope { scope in
            scope.setTag(value: version.contains("-nightly.") ? "nightly" : "stable", key: "channel")
        }
        name()
    }

    /// Names the person by the same one-way hash of their account the Mac's usage ping sends.
    func follow(account: String?) {
        self.account = account
        name()
    }

    private func name() {
        guard SentrySDK.isEnabled else { return }
        SentrySDK.setUser(account.map { id in
            let user = User()
            user.userId = SHA256.hash(data: Data("sikemux-usage|\(id)".utf8)).map { String(format: "%02x", $0) }.joined()
            return user
        })
    }
}
