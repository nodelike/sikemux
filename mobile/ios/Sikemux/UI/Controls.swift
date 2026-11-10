import SwiftUI

enum ButtonKind {
    case primary, neutral, danger, text
}

/// The app's full-width button, as the Mac draws its primary and neutral buttons.
struct ActionButton<Lead: View>: View {
    @Environment(\.palette) private var palette
    @Environment(\.isEnabled) private var enabled
    let title: String
    var kind: ButtonKind = .neutral
    /// Working on what the press started: a spinner takes the icon's place, and the button stays as bright.
    var busy = false
    let action: () -> Void
    @ViewBuilder var lead: Lead

    var body: some View {
        Button(action: action) {
            HStack(spacing: 10) {
                if busy {
                    ProgressView().tint(ink).frame(width: 18, height: 18)
                } else {
                    lead.foregroundStyle(ink)
                }
                Text(title)
                    .font(Theme.ui(16, kind == .text ? .medium : .semibold))
                    .foregroundStyle(ink)
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 8)
            .frame(maxWidth: .infinity, minHeight: kind == .text ? 44 : 50)
            .background(RoundedRectangle(cornerRadius: Radius.row, style: .continuous).fill(fill))
            .overlay(RoundedRectangle(cornerRadius: Radius.row, style: .continuous).strokeBorder(border, lineWidth: 1))
            .contentShape(Rectangle())
        }
        .buttonStyle(PressFade())
        .disabled(busy)
        .opacity(enabled ? 1 : 0.5)
    }

    private var fill: Color {
        switch kind {
        case .primary: palette.ink
        case .neutral, .danger: palette.raised
        case .text: .clear
        }
    }

    private var border: Color {
        switch kind {
        case .primary: palette.ink
        case .neutral, .danger: palette.border
        case .text: .clear
        }
    }

    private var ink: Color {
        switch kind {
        case .primary: palette.ground
        case .neutral: palette.ink
        case .danger: palette.danger
        case .text: palette.secondary
        }
    }
}

extension ActionButton where Lead == AnyView {
    init(_ title: String, kind: ButtonKind = .neutral, busy: Bool = false, icon: IconDef? = nil, action: @escaping () -> Void) {
        self.init(title: title, kind: kind, busy: busy, action: action) {
            AnyView(icon.map { IconView(icon: $0, size: 17) })
        }
    }
}

/// Pressed controls fade a little, as the RN app's did; nothing scales.
struct PressFade: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label.opacity(configuration.isPressed ? 0.75 : 1)
    }
}

struct IconButton: View {
    @Environment(\.palette) private var palette
    let icon: IconDef
    let label: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            IconView(icon: icon, size: 18)
                .foregroundStyle(palette.tertiary)
                .frame(width: 44, height: 44)
                .contentShape(Circle())
        }
        .buttonStyle(PressFade())
        .accessibilityLabel(label)
    }
}

/// A text field on the sunken surface, its border lit while it has the keyboard.
struct Field: View {
    @Environment(\.palette) private var palette
    let placeholder: String
    @Binding var text: String
    var icon: IconDef?
    var secure = false
    var trailingRoom: CGFloat = 14
    var autoFocus = false
    @FocusState private var focused: Bool

    var body: some View {
        HStack(spacing: 0) {
            if let icon {
                IconView(icon: icon, size: 18)
                    .foregroundStyle(focused ? palette.secondary : palette.tertiary)
                    .frame(width: 44)
            }
            Group {
                if secure {
                    SecureField("", text: $text, prompt: prompt)
                } else {
                    TextField("", text: $text, prompt: prompt)
                }
            }
            .focused($focused)
            .font(Theme.ui(16))
            .foregroundStyle(palette.ink)
            .tint(palette.accent)
            .padding(.leading, icon == nil ? 14 : 0)
            .padding(.trailing, trailingRoom)
        }
        .frame(height: 50)
        .background(RoundedRectangle(cornerRadius: Radius.row, style: .continuous).fill(palette.sunken))
        .overlay(
            RoundedRectangle(cornerRadius: Radius.row, style: .continuous)
                .strokeBorder(focused ? palette.borderSelected : palette.border, lineWidth: 1)
        )
        .onAppear { if autoFocus { focused = true } }
    }

    private var prompt: Text {
        Text(placeholder).foregroundStyle(palette.tertiary)
    }
}

struct PasswordField: View {
    @Environment(\.palette) private var palette
    let placeholder: String
    @Binding var text: String
    var autoFocus = false
    @State private var shown = false

    var body: some View {
        Field(placeholder: placeholder, text: $text, icon: Icons.lock, secure: !shown, trailingRoom: 48, autoFocus: autoFocus)
            .overlay(alignment: .trailing) {
                Button { shown.toggle() } label: {
                    IconView(icon: shown ? Icons.eyeOff : Icons.eye, size: 18)
                        .foregroundStyle(palette.tertiary)
                        .frame(width: 48, height: 50)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(shown ? "Hide password" : "Show password")
            }
    }
}

/// A small rounded button, the Mac's chip.
struct Chip: View {
    @Environment(\.palette) private var palette
    let title: String
    var primary = false
    var busy = false
    var disabled = false
    let action: () -> Void

    var body: some View {
        Button {
            Haptics.select()
            action()
        } label: {
            Text(title)
                .font(Theme.ui(13.5, .medium))
                .foregroundStyle(disabled ? palette.inkFaint : primary ? palette.ground : palette.ink)
                .lineLimit(1)
                .padding(.horizontal, 12)
                .frame(minHeight: 32)
                .background(RoundedRectangle(cornerRadius: Radius.control, style: .continuous).fill(primary ? palette.ink : palette.raised))
                .overlay(
                    RoundedRectangle(cornerRadius: Radius.control, style: .continuous)
                        .strokeBorder(primary ? palette.ink : palette.border, lineWidth: 1)
                )
                .opacity(busy ? 0.6 : 1)
        }
        .buttonStyle(PressFade())
        .disabled(busy || disabled)
    }
}

/// The rail's scope track: two or three options, the chosen one lifted.
struct Track<Value: Hashable>: View {
    @Environment(\.palette) private var palette
    struct Option {
        let value: Value
        let label: String
        var count: Int?
    }

    let options: [Option]
    @Binding var value: Value

    var body: some View {
        HStack(spacing: 0) {
            ForEach(options, id: \.value) { option in
                let on = option.value == value
                Button {
                    Haptics.select()
                    value = option.value
                } label: {
                    HStack(spacing: 6) {
                        Text(option.label)
                            .font(Theme.ui(13))
                            .foregroundStyle(on ? palette.ink : palette.tertiary)
                        if let count = option.count, count > 0 {
                            Text("\(count)")
                                .font(Theme.ui(11, .semibold))
                                .foregroundStyle(palette.ground)
                                .padding(.horizontal, 5)
                                .frame(minWidth: 16, minHeight: 16)
                                .background(Capsule().fill(palette.ink))
                        }
                    }
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .background(RoundedRectangle(cornerRadius: 7, style: .continuous).fill(on ? palette.active : .clear))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(on ? [.isSelected] : [])
            }
        }
        .padding(3)
        .frame(minHeight: 36)
        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(palette.raised.opacity(0.8)))
        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(palette.border, lineWidth: 1))
    }
}

/// One of a sheet's actions, on its own raised row.
struct MenuItem: View {
    @Environment(\.palette) private var palette
    let icon: IconDef
    let label: String
    var danger = false
    let action: () -> Void

    var body: some View {
        let ink = danger ? palette.danger : palette.ink
        Button(action: action) {
            HStack(spacing: 12) {
                IconView(icon: icon, size: 18).foregroundStyle(ink)
                Text(label).font(Theme.ui(15.5, .medium)).foregroundStyle(ink)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 14)
            .frame(minHeight: 52)
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(palette.raised))
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(palette.border, lineWidth: 1))
            .contentShape(Rectangle())
        }
        .buttonStyle(MenuItemPress())
    }
}

private struct MenuItemPress: ButtonStyle {
    @Environment(\.palette) private var palette

    func makeBody(configuration: Configuration) -> some View {
        configuration.label.overlay(
            RoundedRectangle(cornerRadius: 14, style: .continuous).fill(configuration.isPressed ? palette.active : .clear)
        )
    }
}

/// Lays its views out in rows, starting another when one is full, as chips wrap.
struct Wrap: Layout {
    var spacing: CGFloat = 8

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let rows = arrange(proposal.width ?? .infinity, subviews)
        return CGSize(width: rows.map(\.width).max() ?? 0, height: rows.last.map { $0.y + $0.height } ?? 0)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        for row in arrange(bounds.width, subviews) {
            var x = bounds.minX
            for index in row.items {
                let size = subviews[index].sizeThatFits(.unspecified)
                subviews[index].place(at: CGPoint(x: x, y: bounds.minY + row.y + (row.height - size.height) / 2), proposal: .unspecified)
                x += size.width + spacing
            }
        }
    }

    private func arrange(_ width: CGFloat, _ subviews: Subviews) -> [(items: [Int], y: CGFloat, width: CGFloat, height: CGFloat)] {
        var rows: [(items: [Int], y: CGFloat, width: CGFloat, height: CGFloat)] = []
        var current: (items: [Int], y: CGFloat, width: CGFloat, height: CGFloat) = ([], 0, 0, 0)
        for (index, subview) in subviews.enumerated() {
            let size = subview.sizeThatFits(.unspecified)
            let needed = current.items.isEmpty ? size.width : current.width + spacing + size.width
            if needed > width, !current.items.isEmpty {
                rows.append(current)
                current = ([], current.y + current.height + spacing, 0, 0)
            }
            current.width = current.items.isEmpty ? size.width : current.width + spacing + size.width
            current.height = max(current.height, size.height)
            current.items.append(index)
        }
        if !current.items.isEmpty { rows.append(current) }
        return rows
    }
}
