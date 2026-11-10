import CoreText
import SwiftUI

/// The app's default colours, which the share sheet always draws in.
let palette = Palette.standard

extension Theme {
    /// The app's fonts, read from the app itself so the extension carries no copy of them.
    static func registerFonts() {
        let app = Bundle.main.bundleURL.deletingLastPathComponent().deletingLastPathComponent()
        for name in ["Figtree_400Regular", "Figtree_500Medium", "Figtree_600SemiBold", "JetBrainsMono_400Regular"] {
            CTFontManagerRegisterFontsForURL(app.appendingPathComponent("\(name).ttf") as CFURL, .process, nil)
        }
    }
}
