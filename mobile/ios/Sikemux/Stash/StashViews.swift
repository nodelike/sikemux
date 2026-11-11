import SwiftUI

/// An item's picture: a link's page picture, a stashed photo, else the icon of its kind.
struct StashFace: View {
    @Environment(\.palette) private var palette
    let item: StashItem
    var size: CGFloat = 46
    @State private var photo: UIImage?

    private var icon: IconDef {
        switch item.kind {
        case .link: Icons.link
        case .text: Icons.text
        case .image, .video: Icons.image
        case .file, .unknown: Icons.file
        case .prompt: Icons.message
        }
    }

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: size / 5, style: .continuous)
        ZStack {
            shape.fill(palette.raised)
            if let photo {
                Image(uiImage: photo).resizable().scaledToFill()
            } else if item.kind == .link, let picture = item.preview?.imageUrl.flatMap(URL.init(string:)) {
                AsyncImage(url: picture) { image in
                    image.resizable().scaledToFill()
                } placeholder: {
                    glyph
                }
            } else {
                glyph
            }
        }
        .frame(width: size, height: size)
        .clipShape(shape)
        .overlay(shape.strokeBorder(photo == nil ? palette.border : .clear, lineWidth: 1))
        .task(id: item.id) {
            guard item.kind == .image, photo == nil else { return }
            do {
                let file = try await StashAPI.download(item)
                photo = await Task.detached { UIImage.thumbnail(of: file, pixels: 160) }.value
            } catch {
                print("sikemux: could not bring a stashed photo down: \(error)")
            }
        }
    }

    private var glyph: some View {
        IconView(icon: icon, size: (size * 0.39).rounded()).foregroundStyle(palette.secondary)
    }

}

/// The composer's way into the stash, counting what came in since the person last looked.
struct StashButton: View {
    @Environment(\.palette) private var palette
    let action: () -> Void

    var body: some View {
        let fresh = StashStore.shared.new.count
        Button(action: action) {
            HStack(spacing: 5) {
                IconView(icon: Icons.stack, size: 16).foregroundStyle(fresh > 0 ? palette.ink : palette.inkDim)
                if fresh > 0 {
                    Text(fresh > 99 ? "99+" : "\(fresh)")
                        .font(Theme.ui(10.5, .semibold)).monospacedDigit()
                        .foregroundStyle(palette.ground)
                        .padding(.horizontal, 5)
                        .frame(minWidth: 17, minHeight: 16)
                        .background(Capsule().fill(palette.accent))
                }
            }
            .padding(.horizontal, 8)
            .frame(minWidth: 34, minHeight: 34)
            .contentShape(Rectangle())
        }
        .buttonStyle(StashPress())
        .accessibilityLabel(fresh > 0 ? "Stash, \(fresh) new" : "Stash")
    }
}

private struct StashPress: ButtonStyle {
    @Environment(\.palette) private var palette

    func makeBody(configuration: Configuration) -> some View {
        configuration.label.background(
            RoundedRectangle(cornerRadius: 7).fill(configuration.isPressed ? palette.active : .clear))
    }
}

/// A link or text riding beside the composer's words until the message is sent.
struct StashChip: View {
    @Environment(\.palette) private var palette
    let chip: ContextChip
    let onRemove: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            ChipFace(uri: chip.uri)
            Text(chip.title).font(Theme.ui(12.5, .medium)).foregroundStyle(palette.ink).lineLimit(1)
        }
        .padding(.leading, 6)
        .padding(.trailing, 28)
        .frame(maxWidth: 220, minHeight: 44, maxHeight: 44, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 9).fill(palette.sunken))
        .overlay(RoundedRectangle(cornerRadius: 9).strokeBorder(palette.border, lineWidth: 1))
        .overlay(alignment: .topTrailing) {
            Button(action: onRemove) {
                IconView(icon: Icons.close, size: 10).foregroundStyle(palette.ground)
                    .frame(width: 18, height: 18)
                    .background(Circle().fill(palette.ink))
                    .padding(3)
                    .contentShape(Rectangle().inset(by: -10))
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Remove \(chip.title)")
        }
    }
}

/// A link's site icon, an earlier chat's agent, or the text glyph for stashed text and a link whose site has none.
private struct ChipFace: View {
    @Environment(\.palette) private var palette
    let uri: String

    var body: some View {
        let host = uri.firstMatch(of: /^https?:\/\/([^\/?#]+)/.ignoresCase())?.1
        let earlier = uri.firstMatch(of: /^sikemux:\/\/earlier\/([^\/]+)\//)?.1
        ZStack {
            if let earlier {
                AgentMark(provider: String(earlier), size: 16)
            } else if let host, let icon = URL(string: "https://\(host)/favicon.ico") {
                AsyncImage(url: icon) { phase in
                    if let image = phase.image {
                        image.resizable().frame(width: 16, height: 16).clipShape(RoundedRectangle(cornerRadius: 3))
                    } else {
                        IconView(icon: Icons.link, size: 13).foregroundStyle(palette.secondary)
                    }
                }
            } else {
                IconView(icon: Icons.text, size: 13).foregroundStyle(palette.secondary)
            }
        }
        .frame(width: 30, height: 30)
        .background(RoundedRectangle(cornerRadius: 6).fill(palette.raised))
        .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(palette.border, lineWidth: 1))
    }
}

/// The stash items an `&` word in the composer names, above the composer as its slash commands show; a tap attaches one.
struct StashMenu: View {
    @Environment(\.palette) private var palette
    let query: String
    let onPick: (StashItem) -> Void

    var body: some View {
        let store = StashStore.shared
        let matches = Array(StashWords.matches(store.items, query).prefix(4))
        VStack(spacing: 4) {
            if matches.isEmpty {
                Text(
                    store.items.isEmpty
                        ? "Nothing stashed. Share to Sikemux from any app to keep it here."
                        : "Nothing in the stash matches \(query)"
                )
                .font(Theme.ui(12)).foregroundStyle(palette.tertiary)
                .frame(maxWidth: .infinity, minHeight: 40, alignment: .leading)
                .padding(.horizontal, 12)
                .background(item)
            } else {
                ForEach(matches) { found in
                    Button {
                        Haptics.select()
                        onPick(found)
                    } label: {
                        HStack(spacing: 10) {
                            StashFace(item: found, size: 34)
                            VStack(alignment: .leading, spacing: 1) {
                                Text(StashWords.title(found)).font(Theme.ui(13.5, .medium)).foregroundStyle(palette.ink)
                                Text(found.note.flatMap { $0.isEmpty ? nil : $0 } ?? StashWords.place(found))
                                    .font(Theme.ui(12)).foregroundStyle(palette.tertiary)
                            }
                            .lineLimit(1)
                            Spacer(minLength: 0)
                        }
                        .padding(.horizontal, 8)
                        .padding(.vertical, 5)
                        .frame(minHeight: 48)
                        .background(item)
                    }
                    .buttonStyle(PressFade())
                    .accessibilityLabel("\(StashWords.title(found)), from the stash")
                }
            }
        }
        .padding(.bottom, 8)
    }

    private var item: some View {
        RoundedRectangle(cornerRadius: 10).fill(palette.overlay)
            .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(palette.border, lineWidth: 1))
    }
}

/// Shown over the composer while its prompt goes to the stash, then with Undo for a while.
struct StashedToast: View {
    @Environment(\.palette) private var palette
    let done: Bool
    let onUndo: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            if done {
                IconView(icon: Icons.check, size: 13).foregroundStyle(palette.live)
            } else {
                ProgressView().controlSize(.small).tint(palette.inkDim)
            }
            Text(done ? "Stashed" : "Stashing…").font(Theme.ui(13, .medium)).foregroundStyle(palette.ink).padding(
                .trailing, 4)
            if done {
                Button(action: onUndo) {
                    Text("Undo").font(Theme.ui(13, .semibold)).foregroundStyle(palette.accent)
                        .padding(.horizontal, 10).frame(height: 26)
                }
                .buttonStyle(StashPress())
                .accessibilityLabel("Undo, put the prompt back")
            }
        }
        .padding(.leading, 12)
        .padding(.trailing, 4)
        .frame(height: 36)
        .background(RoundedRectangle(cornerRadius: 10).fill(palette.overlay))
        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(palette.border, lineWidth: 1))
        .padding(.bottom, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .contain)
    }
}

/// Stashes a composer's prompt and offers Undo for a while after; `toast` is what to show over the composer meanwhile.
@MainActor @Observable
final class PromptStash {
    enum Toast: Equatable {
        case stashing
        case stashed(id: String)
    }

    private let shown = Offer<Toast>()
    var toast: Toast? { shown.current }

    /// `work` stashes the prompt and answers the new item's id; `report` hears a failure in words.
    func stash(_ work: @escaping () async throws -> String, report: @escaping (String) -> Void) {
        Haptics.tap()
        shown.hold(.stashing)
        Task {
            do {
                let id = try await work()
                Haptics.success()
                shown.offer(.stashed(id: id))
            } catch {
                shown.clear()
                report("could not stash it: \(problem(error))")
            }
        }
    }

    /// Puts the prompt back with `undo`, which takes the stashed item's id.
    func undo(_ undo: @escaping (String) async throws -> Void, report: @escaping (String) -> Void) {
        guard case .stashed(let id) = toast else { return }
        shown.clear()
        Task {
            do { try await undo(id) } catch { report("could not put it back: \(problem(error))") }
        }
    }
}
