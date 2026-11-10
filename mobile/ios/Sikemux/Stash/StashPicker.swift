import SwiftUI

/// The stash, newest first, to pick items from for the next message; `chosen` are numbered in the order picked.
/// What came in since the person last looked is marked new, and opening it counts as looking.
struct StashPicker: View {
    @Environment(\.palette) private var palette
    let chosen: [StashItem]
    let onToggle: (StashItem) -> Void
    let onAddNew: ([StashItem]) -> Void
    /// Present while the composer holds something to stash.
    var onStashPrompt: (() -> Void)?
    /// Pulled down from its top.
    let onBack: () -> Void
    let onClose: () -> Void

    @State private var fresh = Set(StashStore.shared.new.map(\.id))
    @State private var removing: StashItem?

    /// How far the list is pulled down past its top before it goes back to the tiles.
    private static let pullDown: CGFloat = 70

    private var store: StashStore { StashStore.shared }

    var body: some View {
        let news = store.items.filter { fresh.contains($0.id) }
        VStack(spacing: 0) {
            HStack(spacing: 0) {
                Button(action: onClose) {
                    IconView(icon: Icons.close, size: 15).foregroundStyle(palette.tertiary).frame(width: 40, height: 40)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Close")
                Text("Stash").font(Theme.ui(16, .semibold)).foregroundStyle(palette.ink).frame(maxWidth: .infinity)
                    .accessibilityAddTraits(.isHeader)
                if news.isEmpty {
                    Color.clear.frame(width: 40, height: 40)
                } else {
                    Button { onAddNew(news) } label: {
                        Text("Add \(news.count) new").font(Theme.ui(14, .semibold)).foregroundStyle(palette.accent)
                            .frame(minWidth: 40, minHeight: 40, alignment: .trailing).padding(.horizontal, 4)
                    }
                    .buttonStyle(.plain)
                }
            }
            .frame(height: 46)
            .padding(.bottom, 6)
            if let onStashPrompt {
                Button(action: onStashPrompt) {
                    HStack(spacing: 10) {
                        IconView(icon: Icons.stack, size: 16).foregroundStyle(palette.ink)
                        Text("Stash this prompt").font(Theme.ui(14.5, .medium)).foregroundStyle(palette.ink)
                        Spacer()
                        Text("Clears the composer").font(Theme.ui(12)).foregroundStyle(palette.tertiary)
                    }
                    .padding(.horizontal, 12)
                    .frame(height: 48)
                    .background(RoundedRectangle(cornerRadius: 10).fill(palette.raised))
                    .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(palette.border, lineWidth: 1))
                }
                .buttonStyle(PressFade())
                .padding(.bottom, 8)
            }
            content
            Color.clear.frame(height: chosen.isEmpty ? 0 : 80)
        }
        .task {
            await store.refresh()
            fresh.formUnion(store.new.map(\.id))
            await store.see()
        }
        .confirmationDialog("Remove from stash?", isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }),
                            titleVisibility: .visible, presenting: removing) { item in
            Button("Remove", role: .destructive) {
                Task {
                    do { try await store.remove(item.id) } catch { Haptics.failure() }
                }
            }
        } message: { item in
            Text(StashWords.title(item))
        }
    }

    @ViewBuilder private var content: some View {
        if !store.items.isEmpty {
            ScrollView(showsIndicators: false) {
                LazyVStack(spacing: 2) {
                    GeometryReader { top in
                        Color.clear.preference(key: PullKey.self, value: top.frame(in: .named("stash")).minY)
                    }
                    .frame(height: 0)
                    ForEach(store.items) { item in
                        StashRow(item: item, fresh: fresh.contains(item.id),
                                 order: (chosen.firstIndex { $0.id == item.id } ?? -1) + 1) {
                            Haptics.select()
                            onToggle(item)
                        } remove: {
                            Haptics.select()
                            removing = item
                        }
                    }
                }
            }
            .coordinateSpace(name: "stash")
            .padding(.horizontal, -4)
            .onPreferenceChange(PullKey.self) { pulled in
                if pulled > Self.pullDown { onBack() }
            }
        } else if store.state == .loading {
            VStack(spacing: 2) {
                ForEach(0..<3, id: \.self) { _ in
                    HStack(spacing: 12) {
                        RoundedRectangle(cornerRadius: 9).fill(palette.raised).frame(width: 46, height: 46)
                        VStack(alignment: .leading, spacing: 8) {
                            RoundedRectangle(cornerRadius: 4).fill(palette.raised).frame(height: 11).containerRelativeFrame(.horizontal) { w, _ in w * 0.5 }
                            RoundedRectangle(cornerRadius: 4).fill(palette.raised).frame(height: 11).containerRelativeFrame(.horizontal) { w, _ in w * 0.3 }
                        }
                        Spacer()
                    }
                    .padding(8)
                    .frame(minHeight: 62)
                }
            }
            .accessibilityLabel("Loading the stash")
        } else if store.state == .offline {
            empty(icon: Icons.globe, title: "Can't reach Sikemux", detail: "Check the phone is online.") {
                Chip(title: "Try again") { Task { await store.refresh() } }.padding(.top, 10)
            }
        } else {
            empty(icon: Icons.stack, title: "Nothing stashed",
                  detail: "Share a link, a photo or a file to Sikemux from any app to keep it here.") { EmptyView() }
        }
    }

    private func empty<Extra: View>(icon: IconDef, title: String, detail: String, @ViewBuilder extra: () -> Extra) -> some View {
        VStack(spacing: 8) {
            PromptGlyph(icon: icon, small: true)
            Text(title).font(Theme.ui(16, .semibold)).foregroundStyle(palette.ink).padding(.top, 8)
            Text(detail).font(Theme.ui(14)).lineSpacing(6).foregroundStyle(palette.tertiary).multilineTextAlignment(.center)
            extra()
        }
        .padding(.top, 70)
        .padding(.horizontal, 36)
        .frame(maxWidth: .infinity)
    }
}

private struct PullKey: PreferenceKey {
    static let defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = nextValue() }
}

private struct StashRow: View {
    @Environment(\.palette) private var palette
    let item: StashItem
    let fresh: Bool
    let order: Int
    let pick: () -> Void
    let remove: () -> Void

    var body: some View {
        let file = item.kind == .file || item.kind == .video
        let prompt = item.kind == .prompt
        HStack(spacing: 12) {
            StashFace(item: item)
            VStack(alignment: .leading, spacing: 0) {
                HStack(alignment: .top, spacing: 6) {
                    Text(prompt ? (item.text ?? "").trimmingCharacters(in: .whitespacesAndNewlines) : StashWords.title(item))
                        .font(file ? Theme.mono(13) : Theme.ui(14.5, .medium))
                        .foregroundStyle(palette.ink)
                        .lineLimit(prompt ? 2 : 1)
                    if fresh {
                        Text("NEW").font(Theme.ui(9.5, .semibold)).tracking(0.76).foregroundStyle(palette.ground)
                            .padding(.horizontal, 5).frame(height: 15)
                            .background(RoundedRectangle(cornerRadius: 4).fill(palette.accent))
                            .padding(.top, 2)
                    }
                }
                if prompt, let attached = item.attachments, !attached.isEmpty {
                    HStack(spacing: 4) {
                        ForEach(attached.prefix(4)) { StashFace(item: $0, size: 24) }
                    }
                    .padding(.top, 5)
                }
                if let note = item.note, !note.isEmpty {
                    Text(note).font(Theme.ui(12.5)).foregroundStyle(palette.secondary).lineLimit(1).padding(.top, 3)
                }
                Text(StashWords.place(item)).font(Theme.ui(12)).foregroundStyle(palette.tertiary).lineLimit(1).padding(.top, 2)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            ZStack {
                Circle().strokeBorder(order > 0 ? palette.accent : palette.borderStrong, lineWidth: 1.5)
                if order > 0 {
                    Circle().fill(palette.accent)
                    Text("\(order)").font(Theme.ui(12, .semibold)).foregroundStyle(palette.ground)
                }
            }
            .frame(width: 22, height: 22)
        }
        .padding(8)
        .frame(minHeight: 62)
        .background(RoundedRectangle(cornerRadius: 10).fill(order > 0 ? palette.active : .clear))
        .contentShape(Rectangle())
        .onTapGesture(perform: pick)
        .onLongPressGesture(perform: remove)
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(order > 0 ? [.isButton, .isSelected] : .isButton)
        .accessibilityHint("Hold to remove it from the stash")
    }
}
