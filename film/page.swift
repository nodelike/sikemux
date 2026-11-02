// Draws a page as a chat shows it, in WebKit with no background, so the film can lay it over the
// place the window keeps for it: the window's own WebKit views are not in its offscreen pictures.
//   swift film/page.swift <page.html> <width> <height> <scale> <out.png>
import AppKit
import WebKit

let args = CommandLine.arguments
guard args.count == 6, let width = Double(args[2]), let height = Double(args[3]),
    let scale = Double(args[4])
else {
    FileHandle.standardError.write("usage: page.swift <page.html> <width> <height> <scale> <out.png>\n".data(using: .utf8)!)
    exit(2)
}
let source = URL(fileURLWithPath: args[1])
let out = URL(fileURLWithPath: args[5])

final class Loader: NSObject, WKNavigationDelegate {
    let size: CGSize
    let scale: Double
    let out: URL

    init(size: CGSize, scale: Double, out: URL) {
        self.size = size
        self.scale = scale
        self.out = out
    }

    func webView(_ view: WKWebView, didFinish navigation: WKNavigation!) {
        // Fonts and any drawing the page does on load settle a moment after it finishes.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) {
            let config = WKSnapshotConfiguration()
            config.rect = CGRect(origin: .zero, size: self.size)
            // The width is in points, which the screen's own density multiplies.
            let screen = view.window?.backingScaleFactor ?? 1
            config.snapshotWidth = NSNumber(value: self.size.width * self.scale / screen)
            view.takeSnapshot(with: config) { image, error in
                guard let image, let tiff = image.tiffRepresentation,
                    let bitmap = NSBitmapImageRep(data: tiff),
                    let png = bitmap.representation(using: .png, properties: [:])
                else {
                    FileHandle.standardError.write("page.swift: \(error?.localizedDescription ?? "no picture")\n".data(using: .utf8)!)
                    exit(1)
                }
                try? png.write(to: self.out)
                exit(0)
            }
        }
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.prohibited)
let frame = NSRect(x: 0, y: 0, width: width, height: height)
let window = NSWindow(contentRect: frame, styleMask: [.borderless], backing: .buffered, defer: false)
window.backgroundColor = .clear
window.isOpaque = false
let view = WKWebView(frame: frame)
view.setValue(false, forKey: "drawsBackground")
let loader = Loader(size: frame.size, scale: scale, out: out)
view.navigationDelegate = loader
window.contentView = view
window.setFrameOrigin(NSPoint(x: -20000, y: -20000))
window.orderBack(nil)
view.loadHTMLString(try String(contentsOf: source, encoding: .utf8), baseURL: source.deletingLastPathComponent())
DispatchQueue.main.asyncAfter(deadline: .now() + 20) {
    FileHandle.standardError.write("page.swift: the page never finished loading\n".data(using: .utf8)!)
    exit(1)
}
app.run()
