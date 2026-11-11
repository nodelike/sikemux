import SwiftUI

/// The core's colour tokens a phone draws with: Aura Noir, the Mac's default theme, until a host publishes its own
/// (DESIGN.md §3).
struct Palette: Equatable, Sendable {
    var sunken = Color(hex: "#09090b")
    var ground = Color(hex: "#0f0f13")
    var raised = Color(hex: "#18181c")
    var overlay = Color(hex: "#1e1e22")
    /// The Mac's composer sits on its darkest surface; this keeps that step above the phone's ground.
    var composer = Color(hex: "#17171b")
    var border = Color(hex: "#27272b")
    var borderStrong = Color(hex: "#3f3f43")
    var rest = Color(hex: "#6a6a6f")

    var ink = Color(hex: "#e7e5ef")
    var secondary = Color(hex: "#b3b2ba")
    var tertiary = Color(hex: "#86858c")
    var inkDim = Color(hex: "#8b8898")
    var inkFaint = Color(hex: "#736f80")

    var active = Color(hex: "rgba(231, 229, 239, 0.10)")
    var selected = Color(hex: "rgba(162, 119, 255, 0.14)")
    var borderSelected = Color(hex: "rgba(162, 119, 255, 0.42)")

    var accent = Color(hex: "#a277ff")
    var accentSoft = Color(hex: "rgba(162, 119, 255, 0.10)")
    var live = Color(hex: "#61ffca")
    var warn = Color(hex: "#ffca85")
    var danger = Color(hex: "#ff6767")
    var cmd = Color(hex: "#ff6ac1")
    var treeSpine = Color(hex: "#5f4a8a")
    var treeTick = Color(hex: "rgba(162, 119, 255, 0.72)")
    var gitAdded = Color(hex: "#81b88b")
    var gitModified = Color(hex: "#e2c08d")
    var gitDeleted = Color(hex: "#d75f47")
    var gitRenamed = Color(hex: "#6c8cd5")
    var toolRead = Color(hex: "#7183c0")
    var toolEdit = Color(hex: "#cdaf86")
    var toolDelete = Color(hex: "#c25e4b")
    var toolRun = Color(hex: "#d966ae")
    var toolMcp = Color(hex: "#6fd6b9")
    /// The pane grain's dots: the raised surface on a dark theme.
    var shaderDot = Color(hex: "#19191e")

    /// Whether the ground is light, so the status bar and keyboard turn dark on it.
    var isLight = false

    static let standard = Palette()

    private static let names: [String: WritableKeyPath<Palette, Color> & Sendable] = [
        "sunken": \.sunken, "ground": \.ground, "raised": \.raised, "overlay": \.overlay, "composer": \.composer,
        "border": \.border, "borderStrong": \.borderStrong, "rest": \.rest, "ink": \.ink, "secondary": \.secondary,
        "tertiary": \.tertiary, "inkDim": \.inkDim, "inkFaint": \.inkFaint, "active": \.active,
        "selected": \.selected, "borderSelected": \.borderSelected, "accent": \.accent, "accentSoft": \.accentSoft,
        "live": \.live, "warn": \.warn, "danger": \.danger, "cmd": \.cmd, "treeSpine": \.treeSpine,
        "treeTick": \.treeTick, "gitAdded": \.gitAdded, "gitModified": \.gitModified, "gitDeleted": \.gitDeleted,
        "gitRenamed": \.gitRenamed, "toolRead": \.toolRead, "toolEdit": \.toolEdit, "toolDelete": \.toolDelete,
        "toolRun": \.toolRun, "toolMcp": \.toolMcp, "shaderDot": \.shaderDot,
    ]

    /// The palette a host published, over the default for any colour it left out.
    init(published: [String: String]?) {
        self.init()
        guard let published else { return }
        for (name, path) in Self.names {
            if let value = published[name], !value.isEmpty { self[keyPath: path] = Color(hex: value) }
        }
        if let ground = published["ground"] { isLight = Color.luminance(of: ground) > 0.5 }
    }

    init() {}
}

private struct PaletteKey: EnvironmentKey {
    static let defaultValue = Palette.standard
}

extension EnvironmentValues {
    /// The palette a screen draws in; a host's screens use that host's.
    var palette: Palette {
        get { self[PaletteKey.self] }
        set { self[PaletteKey.self] = newValue }
    }
}

/// The app's type and the few colours drawn outside any host's palette. Text the app says is Figtree; text the
/// machine says is mono (DESIGN.md §1).
enum Theme {
    static let inkDim = Palette.standard.inkDim

    static func ui(_ size: CGFloat, _ weight: Font.Weight = .regular) -> Font {
        switch weight {
        case .semibold, .bold, .heavy, .black: .custom("Figtree-SemiBold", fixedSize: size)
        case .medium: .custom("Figtree-Medium", fixedSize: size)
        default: .custom("Figtree-Regular", fixedSize: size)
        }
    }

    static func italic(_ size: CGFloat) -> Font { .custom("Figtree-Italic", fixedSize: size) }

    static func mono(_ size: CGFloat) -> Font { .custom("JetBrainsMono-Regular", fixedSize: size) }

    static let title = ui(24, .semibold)
    static let heading = ui(17, .semibold)
    static let body = ui(15)
    static let row = ui(15.5, .medium)
    static let meta = ui(13)
    static let label = ui(13, .semibold)
    static let monoText = mono(12)
}

/// Corner radii, as the Mac's controls, rows, cards and sheets round.
enum Radius {
    static let control: CGFloat = 10
    static let row: CGFloat = 12
    static let card: CGFloat = 14
    static let sheet: CGFloat = 22
}

extension Text {
    /// The title style with the tracking the Mac sets it in.
    func titleStyle(_ palette: Palette) -> some View {
        font(Theme.title).tracking(-0.7).foregroundStyle(palette.ink)
    }

    func headingStyle(_ palette: Palette) -> some View {
        font(Theme.heading).tracking(-0.35).foregroundStyle(palette.ink)
    }

    func bodyStyle(_ palette: Palette) -> some View {
        font(Theme.body).lineSpacing(7).foregroundStyle(palette.secondary)
    }

    func metaStyle(_ palette: Palette) -> some View {
        font(Theme.meta).foregroundStyle(palette.tertiary)
    }
}

extension Color {
    /// A palette colour as the core writes it: `#rgb`, `#rrggbb`, `#rrggbbaa`, `rgb(…)` or `rgba(…)`.
    init(hex: String) {
        let (red, green, blue, alpha) = Color.channels(of: hex) ?? (0, 0, 0, 1)
        self.init(.sRGB, red: red, green: green, blue: blue, opacity: alpha)
    }

    static func channels(of text: String) -> (Double, Double, Double, Double)? {
        var value = text.trimmingCharacters(in: .whitespaces).lowercased()
        if value.hasPrefix("rgb") {
            guard let open = value.firstIndex(of: "("), let close = value.lastIndex(of: ")") else { return nil }
            let parts = value[value.index(after: open)..<close]
                .split(whereSeparator: { $0 == "," || $0 == " " || $0 == "/" })
                .compactMap { Double($0.trimmingCharacters(in: .whitespaces)) }
            guard parts.count >= 3 else { return nil }
            return (parts[0] / 255, parts[1] / 255, parts[2] / 255, parts.count > 3 ? parts[3] : 1)
        }
        if value.hasPrefix("#") { value.removeFirst() }
        if value.count == 3 { value = value.map { "\($0)\($0)" }.joined() }
        guard value.count == 6 || value.count == 8, let number = UInt64(value, radix: 16) else { return nil }
        let rgb = value.count == 8 ? number >> 8 : number
        let alpha = value.count == 8 ? Double(number & 0xff) / 255 : 1
        return (Double((rgb >> 16) & 0xff) / 255, Double((rgb >> 8) & 0xff) / 255, Double(rgb & 0xff) / 255, alpha)
    }

    static func luminance(of text: String) -> Double {
        guard let (red, green, blue, _) = channels(of: text) else { return 0 }
        return 0.2126 * red + 0.7152 * green + 0.0722 * blue
    }

    /// Two colours mixed, `weight` of this one, as the Mac's stylesheet mixes its inks.
    func mixed(with other: Color, _ weight: Double) -> Color {
        let a =
            UIColor(self).cgColor.converted(
                to: CGColorSpace(name: CGColorSpace.sRGB)!, intent: .defaultIntent, options: nil)?.components ?? [
                0, 0, 0, 1,
            ]
        let b =
            UIColor(other).cgColor.converted(
                to: CGColorSpace(name: CGColorSpace.sRGB)!, intent: .defaultIntent, options: nil)?.components ?? [
                0, 0, 0, 1,
            ]
        return Color(
            .sRGB, red: a[0] * weight + b[0] * (1 - weight), green: a[1] * weight + b[1] * (1 - weight),
            blue: a[2] * weight + b[2] * (1 - weight), opacity: 1)
    }
}
