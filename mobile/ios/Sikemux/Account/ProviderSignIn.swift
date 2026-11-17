import AuthenticationServices
import UIKit

/// Signing in with Apple, Google or GitHub. Apple uses the system's own sheet; Google and GitHub open their page in
/// a sheet inside the app that hands back to Clerk when done.
@MainActor
enum ProviderSignIn {
    enum Provider: String {
        case apple, google, github
    }

    struct Cancelled: Error {}

    static func start(_ provider: Provider) async throws {
        switch provider {
        case .apple: try await withApple()
        case .google, .github: try await onTheWeb("oauth_\(provider.rawValue)")
        }
    }

    private static func onTheWeb(_ strategy: String) async throws {
        let redirect = "\(Config.urlScheme)://sso-callback"
        let (started, _) = try await Clerk.call(
            "POST", "v1/client/sign_ins", ["strategy": strategy, "redirect_url": redirect], as: Clerk.SignIn.self
        )
        guard let page = started.firstFactorVerification?.externalVerificationRedirectUrl.flatMap(URL.init(string:))
        else {
            throw Clerk.Problem(code: nil, message: String(localized: "Something went wrong; try again."))
        }
        let back = try await WebSheet.open(page, scheme: Config.urlScheme)
        let nonce = URLComponents(url: back, resolvingAgainstBaseURL: false)?
            .queryItems?.first { $0.name == "rotating_token_nonce" }?.value
        let (signIn, client) = try await Clerk.call(
            "GET", "v1/client/sign_ins/\(started.id)", nonce.map { ["rotating_token_nonce": $0] } ?? [:],
            as: Clerk.SignIn.self
        )
        try await finish(signIn, client)
    }

    private static func withApple() async throws {
        let credential = try await AppleSheet.ask(scopes: [.email, .fullName])
        guard let token = credential.identityToken.flatMap({ String(data: $0, encoding: .utf8) }) else {
            throw Clerk.Problem(code: nil, message: appleFailed)
        }
        let (signIn, client) = try await Clerk.call(
            "POST", "v1/client/sign_ins", ["strategy": "oauth_token_apple", "token": token], as: Clerk.SignIn.self
        )
        try await finish(signIn, client)
    }

    /// Someone new to Sikemux comes back from the provider with nothing to sign in to yet, and the sign-in becomes
    /// a new account.
    private static func finish(_ signIn: Clerk.SignIn, _ client: Clerk.Client?) async throws {
        if signIn.status == .complete, let session = signIn.createdSessionId {
            return try await Account.shared.activate(session: session, client: client)
        }
        guard signIn.firstFactorVerification?.status == .transferable else {
            throw Clerk.Problem(
                code: nil,
                message: String(
                    localized: "This account needs a sign-in step the app does not do yet; sign in on the web first."))
        }
        let (signUp, made) = try await Clerk.call(
            "POST", "v1/client/sign_ups", ["transfer": "true"], as: Clerk.SignUp.self)
        guard let session = signUp.createdSessionId else {
            throw Clerk.Problem(
                code: nil,
                message: String(
                    localized: "The account needs more than this app asks for; finish signing up on the web."))
        }
        try await Account.shared.activate(session: session, client: made)
    }

    static let appleFailed = String(
        localized:
            "Apple did not sign you in. Check this iPhone is signed in to an Apple Account in Settings, then try again."
    )
}

/// Apple's own sign-in sheet.
@MainActor
final class AppleSheet: NSObject, ASAuthorizationControllerDelegate,
    ASAuthorizationControllerPresentationContextProviding
{
    private var answer: CheckedContinuation<ASAuthorizationAppleIDCredential, Error>?
    private static var showing: AppleSheet?

    /// A fresh Apple credential; throws `ProviderSignIn.Cancelled` when the person closes the sheet.
    static func ask(scopes: [ASAuthorization.Scope]) async throws -> ASAuthorizationAppleIDCredential {
        let sheet = AppleSheet()
        showing = sheet
        defer { showing = nil }
        let request = ASAuthorizationAppleIDProvider().createRequest()
        request.requestedScopes = scopes
        let controller = ASAuthorizationController(authorizationRequests: [request])
        controller.delegate = sheet
        controller.presentationContextProvider = sheet
        return try await withCheckedThrowingContinuation { continuation in
            sheet.answer = continuation
            controller.performRequests()
        }
    }

    func presentationAnchor(for controller: ASAuthorizationController) -> ASPresentationAnchor {
        keyWindow()
    }

    func authorizationController(
        controller: ASAuthorizationController, didCompleteWithAuthorization authorization: ASAuthorization
    ) {
        if let credential = authorization.credential as? ASAuthorizationAppleIDCredential {
            answer?.resume(returning: credential)
        } else {
            answer?.resume(throwing: Clerk.Problem(code: nil, message: ProviderSignIn.appleFailed))
        }
        answer = nil
    }

    func authorizationController(controller: ASAuthorizationController, didCompleteWithError error: Error) {
        let cancelled = (error as? ASAuthorizationError)?.code == .canceled
        answer?.resume(
            throwing: cancelled
                ? ProviderSignIn.Cancelled() : Clerk.Problem(code: nil, message: ProviderSignIn.appleFailed))
        answer = nil
    }
}

/// A provider's sign-in page in a sheet over the app, closed when it hands back to `scheme`.
@MainActor
final class WebSheet: NSObject, ASWebAuthenticationPresentationContextProviding {
    private static var showing: (WebSheet, ASWebAuthenticationSession)?

    static func open(_ page: URL, scheme: String) async throws -> URL {
        let sheet = WebSheet()
        defer { showing = nil }
        return try await withCheckedThrowingContinuation { continuation in
            let session = ASWebAuthenticationSession(url: page, callbackURLScheme: scheme) { url, error in
                if let url {
                    continuation.resume(returning: url)
                } else if (error as? ASWebAuthenticationSessionError)?.code == .canceledLogin {
                    continuation.resume(throwing: ProviderSignIn.Cancelled())
                } else {
                    continuation.resume(throwing: error ?? ProviderSignIn.Cancelled())
                }
            }
            session.presentationContextProvider = sheet
            showing = (sheet, session)
            session.start()
        }
    }

    func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        keyWindow()
    }
}

@MainActor
func keyWindow() -> UIWindow {
    UIApplication.shared.connectedScenes
        .compactMap { $0 as? UIWindowScene }
        .flatMap(\.windows)
        .first { $0.isKeyWindow } ?? UIWindow()
}
