import Foundation

/// What this build talks to: "Sikemux Dev" uses Clerk's development instance and the accounts server on the Mac,
/// a production build the real accounts. The two install side by side, each with its own key and paired hosts.
enum Config {
    private struct Release: Decodable {
        struct Accounts: Decodable {
            struct Production: Decodable {
                let clerkPublishableKey: String
                let apiUrl: String
            }
            struct Dev: Decodable {
                let clerkPublishableKey: String
                let apiPort: Int
            }
            let production: Production
            let dev: Dev
        }
        let accounts: Accounts
    }

    private static let release: Release = {
        guard let url = Bundle.main.url(forResource: "release", withExtension: "json"),
              let data = try? Data(contentsOf: url),
              let release = try? JSONDecoder().decode(Release.self, from: data)
        else { fatalError("the app was built without server/protocol/src/release.json") }
        return release
    }()

    static let isDev = (Bundle.main.object(forInfoDictionaryKey: "SikemuxVariant") as? String) != "production"

    static let urlScheme = isDev ? "sikemux-dev" : "sikemux"

    static var clerkPublishableKey: String {
        isDev ? release.accounts.dev.clerkPublishableKey : release.accounts.production.clerkPublishableKey
    }

    /// A development build uses the accounts server on this Mac, or SIKEMUX_API_URL when it is set.
    static var apiUrl: URL {
        if !isDev { return URL(string: release.accounts.production.apiUrl)! }
        if let configured = ProcessInfo.processInfo.environment["SIKEMUX_API_URL"], let url = URL(string: configured) {
            return url
        }
        return URL(string: "http://127.0.0.1:\(release.accounts.dev.apiPort)")!
    }

    static var version: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0.0.0"
    }

    static var build: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "0"
    }
}
