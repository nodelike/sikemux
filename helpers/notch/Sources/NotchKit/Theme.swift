import AppKit
import CoreText
import Observation
import SwiftUI

/// The app's colours and type. The island wears the person's theme through `worn`.
enum Theme {
    static let worn = Worn()

    static var ink: Color { worn.colours.ink }
    static var inkDim: Color { worn.colours.inkDim }
    static var inkFaint: Color { worn.colours.inkFaint }
    static var accent: Color { worn.colours.accent }
    static var accentSoft: Color { worn.colours.accent.opacity(0.13) }
    static var accentLine: Color { worn.colours.accent.opacity(0.32) }
    static var onAccent: Color { worn.colours.onAccent }
    static var live: Color { worn.colours.live }
    static var warn: Color { worn.colours.warn }
    static var command: Color { worn.colours.command }
    static var danger: Color { worn.colours.danger }

    static let raised = Color.white.opacity(0.06)
    static let hover = Color.white.opacity(0.07)
    static let hairline = Color.white.opacity(0.09)
    /// The row the person chose, as a selected capsule is filled.
    static let chosen = Color.white.opacity(0.12)
    /// Reading text a step quieter than the ink, as a card's context reads.
    static var inkSoft: Color { ink.opacity(0.78) }

    static var yolo: LinearGradient {
        LinearGradient(colors: [accent, command, danger, warn, live, accent], startPoint: .leading, endPoint: .trailing)
    }

    private static var registered = false

    /// Loads Figtree from `directory`, or else from the helper app's own resources;
    /// without either the island falls back to the system face.
    static func registerFonts(in directory: String?) {
        guard !registered,
              let directory = directory ?? Bundle.main.resourceURL?.appendingPathComponent("Fonts").path
        else { return }
        registered = true
        let names = (try? FileManager.default.contentsOfDirectory(atPath: directory)) ?? []
        for name in names where name.hasSuffix(".ttf") {
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
    /// The relative luminance of a `#rrggbb` colour, as WCAG measures contrast; none for any other.
    static func luminance(hex: String) -> Double? {
        let text = hex.hasPrefix("#") ? String(hex.dropFirst()) : hex
        guard text.count == 6, let value = UInt64(text, radix: 16) else { return nil }
        func linear(_ channel: UInt64) -> Double {
            let c = Double(channel & 0xff) / 255
            return c <= 0.04045 ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4)
        }
        return 0.2126 * linear(value >> 16) + 0.7152 * linear(value >> 8) + 0.0722 * linear(value)
    }

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

struct Colours: Equatable {
    var ink, inkDim, inkFaint, accent, onAccent, live, warn, command, danger: Color

    /// The person's palette, each colour where it reads on the island's black and Sikemux's own
    /// where it does not. The inks go together, so a light theme's greys never swap their order.
    init(palette: [String: String]) {
        let own = Colours.own
        func reads(_ name: String) -> Color? {
            guard let css = palette[name], let luminance = Color.luminance(hex: css),
                  (luminance + 0.05) / 0.05 >= 4.5
            else { return nil }
            return Color(hex: css)
        }
        let inks = reads("ink").flatMap { ink in
            palette["inkDim"].map { (ink, Color(hex: $0), Color(hex: palette["inkFaint"] ?? $0)) }
        }
        (ink, inkDim, inkFaint) = inks ?? (own.ink, own.inkDim, own.inkFaint)
        let accent = reads("accent")
        self.accent = accent ?? own.accent
        onAccent = accent.flatMap { _ in palette["onAccent"].map { Color(hex: $0) } } ?? own.onAccent
        live = reads("live") ?? own.live
        warn = reads("warn") ?? own.warn
        command = reads("cmd") ?? own.command
        danger = reads("danger") ?? own.danger
    }

    init(ink: Color, inkDim: Color, inkFaint: Color, accent: Color, onAccent: Color, live: Color, warn: Color, command: Color, danger: Color) {
        (self.ink, self.inkDim, self.inkFaint, self.accent, self.onAccent) = (ink, inkDim, inkFaint, accent, onAccent)
        (self.live, self.warn, self.command, self.danger) = (live, warn, command, danger)
    }
}

/// The colours the island draws with, which views follow as the person's theme changes.
@Observable
final class Worn {
    var colours = Colours.own

    func wear(_ palette: [String: String]) {
        let colours = Colours(palette: palette)
        if colours != self.colours { self.colours = colours }
    }
}
