import Foundation

/// Clerk's Frontend API, as a native app speaks it: form posts with `_is_native=1`, the client named by the token
/// Clerk hands back in each answer's Authorization header, kept in the Keychain so the phone stays signed in.
@MainActor
enum Clerk {
    /// Where a sign-in, sign-up, verification or session stands, as Clerk names it; any other word is `other`.
    enum Status: String, Decodable {
        case complete, active, transferable
        case needsFirstFactor = "needs_first_factor"
        case needsSecondFactor = "needs_second_factor"
        case needsClientTrust = "needs_client_trust"
        case other

        init(from decoder: Decoder) throws {
            self = Status(rawValue: try decoder.singleValueContainer().decode(String.self)) ?? .other
        }
    }

    struct Factor: Decodable {
        let strategy: String
        let emailAddressId: String?
        let safeIdentifier: String?
    }

    struct Verification: Decodable {
        let status: Status?
        let externalVerificationRedirectUrl: String?
    }

    struct SignIn: Decodable {
        let id: String
        let status: Status
        let supportedFirstFactors: [Factor]?
        let supportedSecondFactors: [Factor]?
        let firstFactorVerification: Verification?
        let createdSessionId: String?
    }

    struct SignUp: Decodable {
        let id: String
        let status: Status
        let createdSessionId: String?
    }

    struct SessionVerification: Decodable {
        let status: Status
        let supportedFirstFactors: [Factor]?
    }

    struct User: Codable, Equatable {
        struct Email: Codable, Equatable {
            let id: String
            let emailAddress: String
        }
        struct External: Codable, Equatable {
            let provider: String
        }
        let id: String
        let firstName: String?
        let lastName: String?
        let imageUrl: String?
        let hasImage: Bool?
        let primaryEmailAddressId: String?
        let emailAddresses: [Email]
        let externalAccounts: [External]?
        let passwordEnabled: Bool?
    }

    struct Session: Decodable {
        struct Token: Decodable { let jwt: String }
        let id: String
        let status: Status
        let user: User?
        let lastActiveToken: Token?
    }

    struct Client: Decodable {
        let sessions: [Session]
        let lastActiveSessionId: String?

        var active: Session? {
            sessions.first { $0.id == lastActiveSessionId && $0.status == .active }
                ?? sessions.first { $0.status == .active }
        }
    }

    struct Problem: LocalizedError {
        /// Clerk's status, or nil when it could not be reached.
        var status: Int?
        let code: String?
        let message: String
        var errorDescription: String? { message }
    }

    private struct Answer<Response: Decodable>: Decodable {
        let response: Response
        let client: Client?
    }

    private struct Reported: Decodable {
        struct Error: Decodable {
            let code: String?
            let message: String?
            let longMessage: String?
        }
        let errors: [Error]?
    }

    private static let clientItem = "sikemux.clerk-client"

    static let frontendApi: URL = {
        let key = Config.clerkPublishableKey
        let encoded = key.split(separator: "_").last.map(String.init) ?? ""
        let padded = encoded + String(repeating: "=", count: (4 - encoded.count % 4) % 4)
        let host = Data(base64Encoded: padded).flatMap { String(data: $0, encoding: .utf8) } ?? ""
        return URL(string: "https://\(host.trimmingCharacters(in: CharacterSet(charactersIn: "$")))")!
    }()

    private static let decoder: JSONDecoder = {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return decoder
    }()

    static func hasClient() throws -> Bool { try Keychain.app.read(clientItem) != nil }

    static func forget() {
        Keychain.app.delete(clientItem)
    }

    /// Calls the Frontend API, returning what it answered and the client it sent with it.
    static func call<Response: Decodable>(
        _ method: String,
        _ path: String,
        _ form: [String: String] = [:],
        as: Response.Type = Response.self
    ) async throws -> (Response, Client?) {
        var parts = URLComponents(url: frontendApi.appending(path: path), resolvingAgainstBaseURL: false)!
        let extra = method == "GET" ? form.map { URLQueryItem(name: $0.key, value: $0.value) } : []
        parts.queryItems = [URLQueryItem(name: "_is_native", value: "1")] + extra
        var request = URLRequest(url: parts.url!, timeoutInterval: 30)
        request.httpMethod = method
        if let client = try? Keychain.app.read(clientItem) {
            request.setValue(client, forHTTPHeaderField: "Authorization")
        }
        if method != "GET" {
            request.setValue("application/x-www-form-urlencoded", forHTTPHeaderField: "Content-Type")
            request.httpBody = Data(encodeForm(form).utf8)
        }
        let (data, response): (Data, URLResponse)
        do {
            (data, response) = try await URLSession.shared.data(for: request)
        } catch {
            throw Problem(status: nil, code: nil, message: AccountAPI.offline)
        }
        let http = response as? HTTPURLResponse
        if let client = http?.value(forHTTPHeaderField: "Authorization"), !client.isEmpty {
            do {
                try Keychain.app.write(clientItem, client)
            } catch {
                print("sikemux: could not keep Clerk's client in the Keychain: \(error)")
            }
        }
        guard let status = http?.statusCode, (200..<300).contains(status) else {
            let first = (try? decoder.decode(Reported.self, from: data))?.errors?.first
            throw Problem(
                status: http?.statusCode,
                code: first?.code,
                message: first?.longMessage ?? first?.message ?? String(localized: "Something went wrong; try again.")
            )
        }
        // A session's token comes bare; everything else comes as `response` beside the client it changed.
        if let answer = try? decoder.decode(Answer<Response>.self, from: data) {
            return (answer.response, answer.client ?? answer.response as? Client)
        }
        return (try decoder.decode(Response.self, from: data), nil)
    }

    private static func encodeForm(_ form: [String: String]) -> String {
        var allowed = CharacterSet.alphanumerics
        allowed.insert(charactersIn: "-._~")
        return form.map { key, value in
            "\(key)=\(value.addingPercentEncoding(withAllowedCharacters: allowed) ?? value)"
        }.joined(separator: "&")
    }

    static func client() async throws -> Client? {
        try await call("GET", "v1/client", as: Client?.self).0
    }

    /// A session token for the accounts API, which Clerk keeps good for a minute.
    static func token(session: String) async throws -> String {
        let (token, _) = try await call("POST", "v1/client/sessions/\(session)/tokens", as: Session.Token.self)
        return token.jwt
    }

    static func signOut(session: String) async {
        do {
            _ = try await call("POST", "v1/client/sessions/\(session)/remove", as: Session.self)
        } catch {
            print("sikemux: Clerk could not end the session: \(error)")
        }
    }
}
