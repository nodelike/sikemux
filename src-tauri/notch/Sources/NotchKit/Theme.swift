import AppKit
import CoreText
import SwiftUI

/// The app's colours and type. The colours the share page's `base.css` names are in Theme.generated.swift.
enum Theme {
    static let raised = Color.white.opacity(0.06)
    static let hover = Color.white.opacity(0.07)
    static let hairline = Color.white.opacity(0.09)

    static let yolo = LinearGradient(colors: [accent, command, danger, warn, live, accent], startPoint: .leading, endPoint: .trailing)

    private static var registered = false

    /// Loads Figtree from `directory`, or else from the helper app's own resources;
    /// without either the island falls back to the system face.
    static func registerFonts(in directory: String?) {
        guard !registered,
              let directory = directory ?? Bundle.main.resourceURL?.appendingPathComponent("Fonts").path
        else { return }
        registered = true
        let names = ["Figtree_400Regular.ttf", "Figtree_500Medium.ttf", "Figtree_600SemiBold.ttf"]
        for name in names {
            let url = URL(fileURLWithPath: directory).appendingPathComponent(name)
            CTFontManagerRegisterFontsForURL(url as CFURL, .process, nil)
        }
    }

    static func ui(_ size: CGFloat, _ weight: Font.Weight = .regular) -> Font {
        let name: String
        switch weight {
        case .semibold, .bold, .heavy, .black: name = "Figtree-SemiBold"
        case .medium: name = "Figtree-Medium"
        default: name = "Figtree-Regular"
        }
        if NSFont(name: name, size: size) != nil {
            return .custom(name, fixedSize: size)
        }
        return .system(size: size, weight: weight)
    }

    static func mono(_ size: CGFloat, _ weight: Font.Weight = .regular) -> Font {
        .system(size: size, weight: weight, design: .monospaced)
    }
}

extension Color {
    init(hex: String) {
        var text = hex.trimmingCharacters(in: .whitespaces)
        if text.hasPrefix("#") { text.removeFirst() }
        if text.count == 3 { text = text.map { "\($0)\($0)" }.joined() }
        let value = UInt64(text, radix: 16) ?? 0
        self.init(
            .sRGB,
            red: Double((value >> 16) & 0xff) / 255,
            green: Double((value >> 8) & 0xff) / 255,
            blue: Double(value & 0xff) / 255,
            opacity: 1
        )
    }
}
