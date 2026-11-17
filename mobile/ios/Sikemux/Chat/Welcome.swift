import SwiftUI

/// What a chat opens on before its first message, as the Mac's: Sikemux's mark in dots, asking what is being made in the
/// project.
struct Welcome: View {
    @Environment(\.palette) private var palette
    let project: String

    var body: some View {
        VStack(spacing: 24) {
            PosterMark(size: 96)
            greeting
        }
        .frame(maxWidth: .infinity)
        .padding(.horizontal, 22)
    }

    private var greeting: some View {
        let place = "\u{FFFC}"
        let parts = String(localized: "What are we making in \(place)?").components(separatedBy: place)
        let before = parts.first?.trimmingCharacters(in: .whitespaces) ?? ""
        let after = parts.count > 1 ? parts[1] : "?"
        return HStack(spacing: 0) {
            Text(before).font(Theme.ui(19, .medium)).fixedSize()
            IconView(icon: Icons.folder, size: 15).foregroundStyle(palette.live).padding(.leading, 7).padding(
                .trailing, 5)
            Text(project).font(Theme.mono(16)).lineLimit(1).truncationMode(.middle)
            Text(after).font(Theme.ui(19, .medium)).fixedSize()
        }
        .foregroundStyle(palette.ink)
        .accessibilityElement(children: .combine)
    }
}

/// Sikemux's mark as the Mac's welcome draws it: a dither of the accent, its dots churning inside the mark.
struct PosterMark: View {
    @Environment(\.palette) private var palette
    @Environment(\.displayScale) private var scale
    @Environment(\.accessibilityReduceMotion) private var still
    let size: CGFloat
    private let start = Date()

    var body: some View {
        TimelineView(.animation(minimumInterval: 1 / 30, paused: still)) { timeline in
            let time = Float(timeline.date.timeIntervalSince(start))
            IconView(icon: Icons.logo, size: size)
                .foregroundStyle(.white)
                .frame(width: size, height: size)
                .layerEffect(
                    ShaderLibrary.posterMark(
                        .float2(CGSize(width: size, height: size)), .float(Float(scale)), .float(time),
                        .color(palette.accent)),
                    maxSampleOffset: .zero)
        }
        .accessibilityHidden(true)
    }
}
