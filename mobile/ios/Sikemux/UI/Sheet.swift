import SwiftUI

/// A sheet that slides up while the screen behind it dims in place. A tap on the dimmed screen or a drag down closes
/// it. Its content scrolls when it is taller than the screen.
struct SheetModifier<SheetContent: View>: ViewModifier {
    @Environment(\.palette) private var palette
    @Environment(\.accessibilityReduceMotion) private var still
    @Binding var isPresented: Bool
    var tall = false
    var onDismiss: (() -> Void)?
    @ViewBuilder var sheet: SheetContent
    @State private var drag: CGFloat = 0
    @State private var shown = false

    private static var closeDrag: CGFloat { 110 }

    func body(content: Content) -> some View {
        content.overlay {
            ZStack(alignment: .bottom) {
                if shown {
                    Color(hex: "rgba(9, 9, 11, 0.62)")
                        .ignoresSafeArea()
                        .onTapGesture { close() }
                        .accessibilityLabel("Close")
                        .accessibilityAddTraits(.isButton)
                        .transition(.opacity)
                    panel
                        .transition(.move(edge: .bottom))
                }
            }
            .animation(still ? nil : .timingCurve(0.33, 1, 0.68, 1, duration: shown ? 0.26 : 0.18), value: shown)
        }
        .onChange(of: isPresented) { _, presented in
            drag = 0
            shown = presented
        }
        .onAppear { shown = isPresented }
        .onChange(of: shown) { _, now in
            if !now { DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { onDismiss?() } }
        }
    }

    private var panel: some View {
        GeometryReader { proxy in
            VStack(spacing: 0) {
                Spacer(minLength: proxy.safeAreaInsets.top + 24)
                VStack(spacing: 0) {
                    Capsule()
                        .fill(palette.borderStrong)
                        .frame(width: 36, height: 5)
                        .padding(.top, 8)
                        .padding(.bottom, 12)
                    ViewThatFits(in: .vertical) {
                        sheet
                        ScrollView(showsIndicators: false) { sheet }
                    }
                }
                .padding(.horizontal, 16)
                .padding(.bottom, proxy.safeAreaInsets.bottom + 12)
                .frame(maxWidth: .infinity)
                .frame(height: tall ? proxy.size.height * 0.82 : nil, alignment: .top)
                .background(
                    UnevenRoundedRectangle(topLeadingRadius: Radius.sheet, topTrailingRadius: Radius.sheet, style: .continuous)
                        .fill(palette.overlay)
                        .overlay(
                            UnevenRoundedRectangle(topLeadingRadius: Radius.sheet, topTrailingRadius: Radius.sheet, style: .continuous)
                                .stroke(palette.border, lineWidth: 1)
                                .mask(Rectangle().padding(.bottom, -1))
                        )
                        .ignoresSafeArea(edges: .bottom)
                )
                .offset(y: max(0, drag))
                .gesture(
                    DragGesture(minimumDistance: 6)
                        .onChanged { drag = $0.translation.height }
                        .onEnded { value in
                            let speed = value.predictedEndTranslation.height - value.translation.height
                            if value.translation.height > Self.closeDrag || speed > 240 {
                                close()
                            } else {
                                withAnimation(.spring(response: 0.3, dampingFraction: 1)) { drag = 0 }
                            }
                        }
                )
                .accessibilityAddTraits(.isModal)
            }
            .frame(maxHeight: .infinity, alignment: .bottom)
        }
        .ignoresSafeArea(edges: [.top, .bottom])
    }

    private func close() {
        UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
        isPresented = false
    }
}

extension View {
    /// Shows `content` in the app's sheet while `isPresented` holds.
    func sheet<Content: View>(
        shown isPresented: Binding<Bool>, tall: Bool = false, onDismiss: (() -> Void)? = nil,
        @ViewBuilder content: () -> Content
    ) -> some View {
        modifier(SheetModifier(isPresented: isPresented, tall: tall, onDismiss: onDismiss, sheet: content))
    }
}

/// A sheet's section label.
struct SheetLabel: View {
    @Environment(\.palette) private var palette
    let text: String

    init(_ text: String) { self.text = text }

    var body: some View {
        Text(text)
            .font(Theme.ui(13, .semibold))
            .foregroundStyle(palette.tertiary)
            .padding(.top, 14)
            .padding(.bottom, 8)
            .padding(.horizontal, 6)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}
