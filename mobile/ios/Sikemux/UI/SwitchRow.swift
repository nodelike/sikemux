import SwiftUI

/// A boxed row in a sheet with a switch at its end, as the account sheet's settings are drawn.
struct SwitchRow<Detail: View>: View {
    @Environment(\.palette) private var palette
    let icon: IconDef
    let title: String
    @Binding var isOn: Bool
    var disabled = false
    @ViewBuilder var detail: Detail

    var body: some View {
        HStack(spacing: 12) {
            IconView(icon: icon, size: 18).foregroundStyle(palette.secondary)
            VStack(alignment: .leading, spacing: 0) {
                Text(LocalizedStringKey(title)).font(Theme.ui(15, .medium)).tracking(-0.15).foregroundStyle(palette.ink)
                detail
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            Toggle(title, isOn: $isOn)
                .labelsHidden()
                .tint(palette.live)
                .disabled(disabled)
        }
        .rowBox(palette)
    }
}

/// A boxed row like `SwitchRow` that shows a setting's value and opens where it is changed.
struct ValueRow: View {
    @Environment(\.palette) private var palette
    let icon: IconDef
    let title: String
    let value: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 12) {
                IconView(icon: icon, size: 18).foregroundStyle(palette.secondary)
                Text(LocalizedStringKey(title)).font(Theme.ui(15, .medium)).tracking(-0.15).foregroundStyle(palette.ink)
                    .frame(maxWidth: .infinity, alignment: .leading)
                Text(value).font(Theme.ui(15)).foregroundStyle(palette.secondary)
            }
            .rowBox(palette)
            .contentShape(Rectangle())
        }
        .buttonStyle(PressFade())
        .accessibilityAddTraits(.isLink)
    }
}

extension View {
    fileprivate func rowBox(_ palette: Palette) -> some View {
        padding(.horizontal, 10)
            .padding(.vertical, 6)
            .frame(minHeight: 52)
            .background(RoundedRectangle(cornerRadius: 9, style: .continuous).fill(palette.raised))
            .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous).strokeBorder(palette.border, lineWidth: 1))
            .padding(.top, 8)
    }
}

extension SwitchRow where Detail == EmptyView {
    init(icon: IconDef, title: String, isOn: Binding<Bool>, disabled: Bool = false) {
        self.init(icon: icon, title: title, isOn: isOn, disabled: disabled) { EmptyView() }
    }
}
