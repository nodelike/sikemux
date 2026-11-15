import SwiftUI

/// Where each row that needs the person sits in the host's list, in the list's own space.
struct NeedsYouRows: PreferenceKey {
    static let defaultValue: [String: CGRect] = [:]
    static func reduce(value: inout [String: CGRect], nextValue: () -> [String: CGRect]) {
        value.merge(nextValue()) { $1 }
    }
}

extension View {
    /// Tells the list where this row sits while it needs the person, so the list's edge can point to it once scrolled away.
    func needsYou(_ id: String, _ on: Bool) -> some View {
        background {
            if on {
                GeometryReader { geometry in
                    Color.clear.preference(key: NeedsYouRows.self, value: [id: geometry.frame(in: .named("list"))])
                }
            }
        }
        .id(id)
    }
}

/// The rows past the part of the list on screen, nearest first.
struct Unseen: Equatable {
    var above: [String] = []
    var below: [String] = []

    init() {}

    init(_ rows: [String: CGRect], top: CGFloat, bottom: CGFloat) {
        guard bottom > top else { return }
        above = rows.filter { $0.value.maxY <= top }.sorted { $0.value.minY > $1.value.minY }.map(\.key)
        below = rows.filter { $0.value.minY >= bottom }.sorted { $0.value.minY < $1.value.minY }.map(\.key)
    }
}

/// The chip at an edge of the list, counting the rows past it that need the person, as the Mac's rail draws it.
struct NeedsYouChip: View {
    @Environment(\.palette) private var palette
    let count: Int
    let up: Bool
    let action: () -> Void

    var body: some View {
        Button {
            Haptics.select()
            action()
        } label: {
            HStack(spacing: 4) {
                IconView(icon: up ? Icons.arrowUp : Icons.arrowDown, size: 11)
                Text("Needs you")
                if count > 1 { Text(count, format: .number).monospacedDigit() }
            }
            .font(Theme.ui(12, .medium))
            .foregroundStyle(palette.warn)
            .padding(.horizontal, 10)
            .frame(height: 24)
            .background(Capsule().fill(palette.warn.opacity(0.2)))
            .background(Capsule().fill(palette.raised))
            .shadow(color: .black.opacity(0.3), radius: 6, y: 2)
        }
        .buttonStyle(PressFade())
        .accessibilityLabel(Text("Needs you"))
    }
}
