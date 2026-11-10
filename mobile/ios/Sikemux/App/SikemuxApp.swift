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
    @Environment(\.scenePhase) private var scene
    @Bindable private var router = Router.shared
    private let account = Account.shared
    private let hub = Hub.shared
    private let session = AccountSession.shared
    private let lock = Lock.shared
    private let network = Network.shared

    var body: some View {
        ZStack {
            palette.ground.ignoresSafeArea()
            if let required = network.updateRequired {
                UpdateRequiredScreen(required: required)
            } else {
                screens
            }
        }
        .alert("New device on your account", isPresented: newDeviceShown, presenting: session.newDevice) { device in
            Button("Keep", role: .cancel) {}
            Button("Remove", role: .destructive) { Task { try? await AccountCalls.removeDevice(device.key) } }
        } message: { device in
            Text(device.name)
        }
        .onChange(of: account.state, initial: true) { _, state in
            session.follow(account.userId)
            if state != .loading { lock.follow(signedIn: account.isSignedIn) }
        }
        .onChange(of: scene) { _, phase in
            if phase == .active { session.foreground() } else if phase == .background { session.background() }
        }
        .onChange(of: network.updateRequired) { _, required in
            if required != nil { Task { await Identity.shared.goOffline() } }
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

    @ViewBuilder
    private var screens: some View {
        switch account.state {
        case .loading:
            EmptyView()
        case .signedOut:
            if let farewell = session.farewell {
                FarewellScreen(farewell: farewell)
            } else {
                NavigationStack {
                    WelcomeScreen()
                        .navigationDestination(for: Route.self) { ScreenFor(route: $0) }
                }
            }
        case .signedIn:
            if !lock.opened {
                if lock.on { LockScreen() }
            } else {
                if case .deleting(let deletesAt) = session.status {
                    RestoreAccountScreen(deletesAt: deletesAt)
                } else if hub.loaded {
                    NavigationStack(path: $router.path) {
                        DevicesScreen()
                            .navigationDestination(for: Route.self) { ScreenFor(route: $0) }
                    }
                }
                LockCover()
            }
        }
    }

    private var newDeviceShown: Binding<Bool> {
        Binding(get: { session.newDevice != nil }, set: { if !$0 { session.newDevice = nil } })
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
