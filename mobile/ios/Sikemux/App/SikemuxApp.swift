import SwiftUI
import UIKit

@main
struct SikemuxApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(\.palette, .standard)
                .preferredColorScheme(.dark)
                .onOpenURL { Router.shared.open($0) }
        }
    }
}

/// The screens, once the account and the paired hosts are known.
struct RootView: View {
    @Environment(\.palette) private var palette
    @Bindable private var router = Router.shared
    private let account = Account.shared
    private let hub = Hub.shared

    var body: some View {
        ZStack {
            palette.ground.ignoresSafeArea()
            switch account.state {
            case .loading:
                EmptyView()
            case .signedOut:
                NavigationStack {
                    WelcomeScreen()
                        .navigationDestination(for: Route.self) { ScreenFor(route: $0) }
                }
            case .signedIn:
                if hub.loaded {
                    NavigationStack(path: $router.path) {
                        DevicesScreen()
                            .navigationDestination(for: Route.self) { ScreenFor(route: $0) }
                    }
                }
            }
        }
        .task {
            Task { _ = await Network.shared.relays() }
            await account.load()
            await hub.reloadDevices()
        }
        .notificationsOffer()
        .task(id: account.isSignedIn) {
            guard account.isSignedIn else { return }
            ShareExtension.signIn()
            StashStore.shared.start()
            try? await Notify.shared.syncPushToken()
        }
    }
}

/// The screen each route opens.
struct ScreenFor: View {
    let route: Route

    var body: some View {
        switch route {
        case .device(let core, let tab): DeviceScreen(core: core, linkedTab: tab)
        case .chat(let core, let agent, let started): ChatScreen(core: core, agentId: agent, started: started)
        case .newChat(let core, let project, let launcher): NewChatScreen(core: core, project: project, launcher: launcher)
        case .pairing(let core): PairingScreen(core: core)
        case .join(let core, let name, let arrived): JoinScreen(core: core, name: name ?? "your host", arrived: arrived)
        case .hostSignIn: HostSignInScreen()
        case .deleteAccount: DeleteAccountScreen()
        case .share(let id, let key): ShareScreen(id: id, key: key)
        case .stash: StashScreen()
        case .signIn: SignInScreen()
        }
    }
}

/// With the navigation bar hidden, UIKit stops the swipe back; this keeps it, from anywhere on the screen as the
/// RN app's stack allowed.
extension UINavigationController: @retroactive UIGestureRecognizerDelegate {
    override open func viewDidLoad() {
        super.viewDidLoad()
        interactivePopGestureRecognizer?.delegate = self
        if #available(iOS 26, *) { interactiveContentPopGestureRecognizer?.delegate = self }
    }

    public func gestureRecognizerShouldBegin(_ gestureRecognizer: UIGestureRecognizer) -> Bool {
        viewControllers.count > 1
    }
}
