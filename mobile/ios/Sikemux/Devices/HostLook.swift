import SikemuxCore
import SwiftUI

/// What a host draws behind its panes: the grain, or a picture saved on the phone.
struct HostBackdrop: Equatable {
    var texture: Bool
    var image: URL?
    var look: BackdropLook?
}

extension EnvironmentValues {
    @Entry var hostBackdrop: HostBackdrop?
    /// The host a screen belongs to.
    @Entry var hostCore = ""
}

/// A host's theme and backdrop as it publishes them, or as last seen, kept with the paired host so its screens open
/// in them before it connects. Its backdrop picture is fetched only when it changes.
@MainActor
enum HostLook {
    private static var palettes: [[String: String]: Palette] = [:]
    /// Hosts fetching a picture now; each fetches one at a time.
    private static var fetching: Set<String> = []
    /// Pictures a host said it does not have; asking again on every change of its view would not help.
    private static var missing: Set<String> = []
    private static var failedAt: [String: Date] = [:]
    /// The picture each host's view names now, to tell a fetch that finished after its host moved on.
    private static var latest: [String: String] = [:]
    private static let retryFailed: TimeInterval = 60

    static func palette(_ live: HostLive, _ device: PairedDevice?) -> Palette {
        let published = live.view?.workspace.palette
        guard let colours = published?.isEmpty == false ? published : device?.palette else { return .standard }
        if let made = palettes[colours] { return made }
        if palettes.count > 16 { palettes.removeAll() }
        let made = Palette(published: colours)
        palettes[colours] = made
        return made
    }

    static func backdrop(_ live: HostLive, _ device: PairedDevice?) -> HostBackdrop {
        let published = live.view?.workspace.backdrop
        let remembered = device?.backdrop
        let current = published.map { $0.image } ?? remembered?.image?.id
        let image = current.flatMap { id in remembered?.image.flatMap { $0.id == id ? saved(live.core, $0) : nil } }
        return HostBackdrop(
            texture: published?.texture ?? remembered?.texture ?? false, image: image,
            look: published?.look ?? remembered?.look)
    }

    /// Keeps what the host publishes with the paired host, and saves a new backdrop picture.
    static func keep(_ live: HostLive, _ device: PairedDevice?) {
        guard let device, let view = live.view else { return }
        let core = live.core
        let colours = view.workspace.palette
        if !colours.isEmpty, colours != device.palette {
            Task { await Hub.shared.update(core) { $0.palette = colours } }
        }
        let texture = view.workspace.backdrop.texture
        let wanted = view.workspace.backdrop.image
        let look = view.workspace.backdrop.look
        let remembered = device.backdrop
        if let wanted, wanted != remembered?.image?.id {
            fetch(live, wanted: wanted, texture: texture, look: look, remembered: remembered)
            return
        }
        if wanted == nil, let image = remembered?.image { delete(core, image) }
        if texture != remembered?.texture || look != remembered?.look || (wanted == nil && remembered?.image != nil) {
            let image = wanted == nil ? nil : remembered?.image
            Task { await Hub.shared.update(core) { $0.backdrop = .init(texture: texture, image: image, look: look) } }
        }
    }

    /// Deletes the picture saved for a host's backdrop.
    static func forget(_ device: PairedDevice) {
        if let image = device.backdrop?.image { delete(device.core, image) }
    }

    private static func fetch(
        _ live: HostLive, wanted: String, texture: Bool, look: BackdropLook?, remembered: PairedDevice.Backdrop?
    ) {
        let core = live.core
        let key = "\(core)/\(wanted)"
        latest[core] = wanted
        guard let connection = live.connection, !fetching.contains(core), !missing.contains(key),
            Date().timeIntervalSince(failedAt[key] ?? .distantPast) > retryFailed
        else { return }
        fetching.insert(core)
        Task {
            defer { fetching.remove(core) }
            do {
                guard let path = try await connection.saveBackdrop(dir: folder(core).path, id: wanted) else {
                    missing.insert(key)
                    return
                }
                let file = URL(filePath: path)
                guard latest[core] == wanted else {
                    delete(file)
                    return
                }
                failedAt[key] = nil
                if let old = remembered?.image { delete(core, old) }
                await Hub.shared.update(core) {
                    $0.backdrop = .init(
                        texture: texture, image: .init(id: wanted, uri: file.absoluteString), look: look)
                }
            } catch {
                failedAt[key] = Date()
            }
        }
    }

    private static func folder(_ core: String) -> URL {
        URL.documentsDirectory.appending(path: "backdrops").appending(path: String(core.prefix(16)))
    }

    /// iOS moves an app's documents on every update, so a saved picture is looked for by its name in the host's
    /// folder first.
    private static func saved(_ core: String, _ image: PairedDevice.Backdrop.Image) -> URL? {
        let name = image.uri.split(separator: "/").last.map(String.init) ?? image.uri
        let here = folder(core).appending(path: name)
        return FileManager.default.fileExists(atPath: here.path) ? here : nil
    }

    private static func delete(_ core: String, _ image: PairedDevice.Backdrop.Image) {
        if let file = saved(core, image) { delete(file) }
    }

    private static func delete(_ file: URL) {
        do {
            try FileManager.default.removeItem(at: file)
        } catch {
            print("sikemux: could not delete a host's backdrop: \(error)")
        }
    }
}

/// A host's screens: drawn in its theme and over its backdrop, with its agents' faces, while this phone keeps it
/// paired; once it is forgotten they go back to Devices.
struct HostScoped<Content: View>: View {
    let core: String
    @ViewBuilder var content: Content
    private let hub = Hub.shared

    var body: some View {
        let live = hub.live(core)
        let device = hub.device(core)
        let palette = HostLook.palette(live, device)
        content
            .environment(\.palette, palette)
            .environment(\.hostCore, core)
            .preferredColorScheme(palette.isLight ? .light : .dark)
            .environment(\.hostBackdrop, HostLook.backdrop(live, device))
            .environment(\.faces, Faces(live.view))
            .onChange(of: live.view?.workspace, initial: true) { HostLook.keep(live, hub.device(core)) }
            .onChange(of: device == nil) { _, gone in
                if gone, hub.loaded { Router.shared.root() }
            }
    }
}

/// Holds a host's connection while the view shows.
private struct Watching: ViewModifier {
    let core: String
    @State private var watch: HostWatch?

    func body(content: Content) -> some View {
        content
            .onAppear { watch = Hub.shared.watch(core) }
            .onDisappear {
                watch?.cancel()
                watch = nil
            }
    }
}

extension View {
    /// Keeps the host connected while this shows, as every screen of a host does.
    func watching(_ core: String) -> some View {
        modifier(Watching(core: core))
    }
}
