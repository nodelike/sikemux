import Photos
import SwiftUI

/// Where a message's photos and files come from: the camera, the newest photos in a grid that opens into the whole
/// library, or the Files app; or, opened from the composer's Stash button, the stash.
struct AttachSheet: ViewModifier {
    @Environment(\.palette) private var palette
    @Binding var shown: Bool
    /// Opens on the stash rather than the photos and files.
    let stash: Bool
    /// How many more files the message takes.
    let room: Int
    let onAttach: ([Attachment]) -> Void
    let onStash: ([StashItem]) -> Void
    /// Stashes what the composer holds as one prompt; absent while it holds nothing.
    let onStashPrompt: (() -> Void)?
    let onProblem: (String) -> Void

    @State private var library = PhotoLibrary()
    @State private var full = false
    @State private var chosen: [PHAsset] = []
    @State private var stashing = false
    @State private var stashChosen: [StashItem] = []
    /// A system screen to show once the sheet has gone.
    @State private var outside: (@MainActor () async throws -> [Attachment])?

    private static let columns = 4
    private static let inset: CGFloat = 2
    private static let peekRows: CGFloat = 2.5

    func body(content: Content) -> some View {
        content
            .liftedSheet(shown: $shown, tall: full || stashing, onDismiss: dismissed) {
                if stashing {
                    StashPicker(
                        chosen: stashChosen,
                        onToggle: toggleStash,
                        onAddNew: { items in
                            onStash(Array(items.prefix(room)))
                            shown = false
                        },
                        onStashPrompt: onStashPrompt.map { prompt in {
                            shown = false
                            prompt()
                        } },
                        onBack: { shown = false },
                        onClose: { shown = false }
                    )
                    .overlay(alignment: .bottom) {
                        if !stashChosen.isEmpty {
                            ActionButton("Add \(stashChosen.count)", kind: .primary) {
                                onStash(stashChosen)
                                shown = false
                            }
                        }
                    }
                } else {
                    photos
                }
            }
            .onChange(of: shown) { _, now in
                if now {
                    full = false
                    chosen = []
                    stashing = stash
                    stashChosen = []
                    if !stash { library.open() }
                } else {
                    library.close()
                }
            }
    }

    @ViewBuilder
    private var photos: some View {
        let gallery = library.access != .denied
        VStack(spacing: 0) {
            if full {
                HStack {
                    Button { shown = false } label: {
                        IconView(icon: Icons.close, size: 15).foregroundStyle(palette.tertiary).frame(width: 40, height: 40)
                    }
                    .buttonStyle(PressFade())
                    .accessibilityLabel("Close")
                    Spacer()
                    Text("Recents").font(Theme.ui(16, .semibold)).foregroundStyle(palette.ink)
                    Spacer()
                    Color.clear.frame(width: 40, height: 40)
                }
                .frame(height: 46)
                .padding(.bottom, 6)
            } else {
                tiles
            }
            if library.access == .limited {
                HStack {
                    Spacer()
                    Button("Manage") { library.manage() }
                        .font(Theme.ui(13, .semibold))
                        .foregroundStyle(palette.accent)
                }
                .frame(height: 30)
                .padding(.horizontal, 4)
            }
            if gallery {
                grid
            } else {
                HStack {
                    Text("No photo access").font(Theme.ui(14, .medium)).foregroundStyle(palette.secondary)
                    Spacer()
                    if library.canAsk {
                        Chip(title: "Allow access", primary: true) { library.open() }
                    } else {
                        Chip(title: "Open Settings") {
                            if let url = URL(string: UIApplication.openSettingsURLString) { UIApplication.shared.open(url) }
                        }
                    }
                }
                .padding(.horizontal, 4)
                .padding(.top, 4)
            }
        }
        .overlay(alignment: .bottom) {
            if !chosen.isEmpty {
                ActionButton("Add \(chosen.count) \(Format.counted("photo", chosen.count))", kind: .primary) {
                    let picked = chosen
                    shown = false
                    attach { try await Pick.library(picked) }
                }
            }
        }
    }

    private var tiles: some View {
        HStack(spacing: 10) {
            tile(Icons.camera, "Camera") { leave { try await Pick.camera() } }
            tile(Icons.image, "Photos") {
                if library.sees {
                    withAnimation(.snappy) { full = true }
                } else if library.canAsk {
                    library.open()
                } else {
                    let room = room
                    leave { try await Pick.photos(limit: room) }
                }
            }
            tile(Icons.file, "Files") {
                let room = room
                leave { try await Pick.files(limit: room) }
            }
        }
        .padding(.horizontal, 4)
        .padding(.top, 2)
        .frame(height: 96, alignment: .top)
    }

    private func tile(_ icon: IconDef, _ label: String, _ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            VStack(spacing: 6) {
                IconView(icon: icon, size: 20)
                    .foregroundStyle(palette.ink)
                    .frame(maxWidth: .infinity, minHeight: 54, maxHeight: 54)
                    .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(palette.raised))
                    .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(palette.border, lineWidth: 1))
                Text(label).font(Theme.ui(12.5, .medium)).foregroundStyle(palette.secondary)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(PressFade())
        .accessibilityLabel(label)
    }

    private var grid: some View {
        GeometryReader { proxy in
            let cell = proxy.size.width / CGFloat(Self.columns)
            let columns = Array(repeating: GridItem(.fixed(cell), spacing: 0), count: Self.columns)
            ScrollView(showsIndicators: false) {
                LazyVGrid(columns: columns, spacing: 0) {
                    if !full {
                        cameraCell.frame(width: cell, height: cell)
                    }
                    ForEach(library.photos, id: \.localIdentifier) { asset in
                        PhotoCell(
                            asset: asset, side: cell,
                            order: (chosen.firstIndex(of: asset) ?? -1) + 1,
                            choosing: !chosen.isEmpty
                        ) { toggle(asset) }
                        .onAppear { if asset == library.photos.last { library.more() } }
                    }
                }
                .padding(.bottom, chosen.isEmpty ? 0 : 80)
            }
            .scrollDisabled(!full)
            .simultaneousGesture(
                DragGesture(minimumDistance: 12).onEnded { drag in
                    if !full && drag.translation.height < -40 { withAnimation(.snappy) { full = true } }
                }
            )
        }
        .frame(height: full ? nil : UIScreen.main.bounds.width / CGFloat(Self.columns) * Self.peekRows)
        .padding(.horizontal, -Self.inset)
    }

    private var cameraCell: some View {
        Button { leave { try await Pick.camera() } } label: {
            VStack(spacing: 6) {
                IconView(icon: Icons.camera, size: 20).foregroundStyle(palette.ink)
                Text("Camera").font(Theme.ui(12, .medium)).foregroundStyle(palette.ink)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(palette.raised))
            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(palette.border, lineWidth: 1))
            .padding(Self.inset)
        }
        .buttonStyle(ActiveFill(radius: 8))
        .accessibilityLabel("Take a photo")
    }

    private func toggle(_ asset: PHAsset) {
        Haptics.select()
        if let at = chosen.firstIndex(of: asset) {
            chosen.remove(at: at)
        } else if chosen.count < room {
            chosen.append(asset)
        }
    }

    private func toggleStash(_ item: StashItem) {
        if let at = stashChosen.firstIndex(where: { $0.id == item.id }) {
            stashChosen.remove(at: at)
        } else if stashChosen.count < room {
            stashChosen.append(item)
        }
    }

    /// iOS shows a system screen only once the sheet over the app has gone.
    private func leave(_ pick: @escaping @MainActor () async throws -> [Attachment]) {
        outside = pick
        shown = false
    }

    private func dismissed() {
        guard let pick = outside else { return }
        outside = nil
        attach(pick)
    }

    private func attach(_ pick: @escaping @MainActor () async throws -> [Attachment]) {
        Task {
            do {
                let picked = try await pick()
                if !picked.isEmpty { onAttach(picked) }
            } catch {
                onProblem("could not pick: \(problem(error))")
            }
        }
    }
}

/// A photo in the gallery, numbered in the order it was chosen.
private struct PhotoCell: View {
    @Environment(\.palette) private var palette
    let asset: PHAsset
    let side: CGFloat
    /// Its place among the chosen, from 1, or 0 when it is not chosen.
    let order: Int
    let choosing: Bool
    let onToggle: () -> Void
    @State private var image: UIImage?

    var body: some View {
        Button(action: onToggle) {
            ZStack(alignment: .topTrailing) {
                Group {
                    if let image {
                        Image(uiImage: image).resizable().scaledToFill()
                    } else {
                        palette.raised
                    }
                }
                .frame(width: side - 4, height: side - 4)
                .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(order > 0 ? palette.accent : .clear, lineWidth: 2))
                if choosing {
                    ZStack {
                        Circle().fill(order > 0 ? palette.accent : Color(hex: "rgba(9, 9, 11, 0.25)"))
                        Circle().strokeBorder(order > 0 ? palette.accent : Color.white.opacity(0.9), lineWidth: 1.5)
                        if order > 0 { Text("\(order)").font(Theme.ui(12, .semibold)).foregroundStyle(palette.ground) }
                    }
                    .frame(width: 22, height: 22)
                    .padding(order > 0 ? 4 : 5)
                }
            }
            .frame(width: side, height: side)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Photo")
        .accessibilityAddTraits(order > 0 ? .isSelected : [])
        .task(id: asset.localIdentifier) {
            let pixels = side * UIScreen.main.scale
            let options = PHImageRequestOptions()
            options.deliveryMode = .opportunistic
            options.isNetworkAccessAllowed = true
            for await found in Self.images(asset, CGSize(width: pixels, height: pixels), options) { image = found }
        }
    }

    private static func images(_ asset: PHAsset, _ size: CGSize, _ options: PHImageRequestOptions) -> AsyncStream<UIImage> {
        AsyncStream { stream in
            let id = PHCachingImageManager.default().requestImage(for: asset, targetSize: size, contentMode: .aspectFill, options: options) { image, info in
                if let image { stream.yield(image) }
                if (info?[PHImageResultIsDegradedKey] as? Bool) != true { stream.finish() }
            }
            stream.onTermination = { _ in PHImageManager.default().cancelImageRequest(id) }
        }
    }
}

extension View {
    /// The attach sheet over this view while `shown` holds.
    func attachSheet(
        shown: Binding<Bool>, stash: Bool, room: Int, onAttach: @escaping ([Attachment]) -> Void,
        onStash: @escaping ([StashItem]) -> Void, onStashPrompt: (() -> Void)?, onProblem: @escaping (String) -> Void
    ) -> some View {
        modifier(AttachSheet(
            shown: shown, stash: stash, room: room, onAttach: onAttach, onStash: onStash,
            onStashPrompt: onStashPrompt, onProblem: onProblem
        ))
    }
}
