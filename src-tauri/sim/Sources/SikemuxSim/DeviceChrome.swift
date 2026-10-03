import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

/// The device around a simulator's screen, drawn from the same artwork Xcode's own simulator window
/// uses: the bezel from `/Library/Developer/DeviceKit/Chrome`, the side buttons at their places on it,
/// and the screen's rounded shape from the device type.
enum DeviceChrome {
    private static let chromeDirectory = URL(fileURLWithPath: "/Library/Developer/DeviceKit/Chrome")
    private static let deviceTypes = URL(fileURLWithPath: "/Library/Developer/CoreSimulator/Profiles/DeviceTypes")
    /// Pixels per point in the images, enough to stay sharp on a Retina display.
    private static let scale: CGFloat = 2

    struct Layout {
        /// The whole picture, buttons included, in points.
        let size: CGSize
        /// Where the screen sits in it, in points.
        let screen: CGRect
    }

    /// `deviceType` is the device type's name, which is also the name of its folder, as in `iPhone 18 Pro`.
    static func render(deviceType: String, screen: CGSize, chrome chromeURL: URL, mask maskURL: URL) throws -> Layout {
        let resources = deviceTypes.appendingPathComponent("\(deviceType).simdevicetype/Contents/Resources")
        let profile = try plist(resources.appendingPathComponent("profile.plist"))
        guard let identifier = profile["chromeIdentifier"] as? String, let name = identifier.split(separator: ".").last else {
            throw Failure(reason: "chrome", message: "\(deviceType) names no device chrome")
        }
        let art = chromeDirectory.appendingPathComponent("\(name).devicechrome/Contents/Resources")
        let manifest = try JSONSerialization.jsonObject(with: Data(contentsOf: art.appendingPathComponent("chrome.json"))) as? [String: Any] ?? [:]
        let images = manifest["images"] as? [String: Any] ?? [:]
        let pdf = { (key: String) in (images[key] as? String).flatMap { PDF(art.appendingPathComponent("\($0).pdf")) } }

        let bezel = bezelWidth(composite: pdf("composite"), images: images, screen: screen)
        let room = images["devicePadding"] as? [String: Any] ?? [:]
        let side = { (key: String) in CGFloat((room[key] as? NSNumber)?.doubleValue ?? 0) }
        let device = CGRect(x: side("left"), y: side("top"), width: screen.width + bezel * 2, height: screen.height + bezel * 2)
        let size = CGSize(width: device.maxX + side("right"), height: device.maxY + side("bottom"))
        let screenRect = device.insetBy(dx: bezel, dy: bezel)

        try draw(size: size, to: chromeURL) { context in
            let buttons = (manifest["inputs"] as? [[String: Any]] ?? []).filter { $0["type"] as? String == "button" }
            for button in buttons where button["onTop"] as? Bool != true { drawButton(button, art: art, device: device, in: context) }
            if let composite = pdf("composite") {
                composite.draw(in: device, context: context)
            } else {
                drawNineSlice(pdf: pdf, device: device, in: context)
            }
            for button in buttons where button["onTop"] as? Bool == true { drawButton(button, art: art, device: device, in: context) }
        }

        if let maskName = profile["framebufferMask"] as? String, let mask = PDF(resources.appendingPathComponent("\(maskName).pdf")) {
            try draw(size: screen, to: maskURL) { context in mask.draw(in: CGRect(origin: .zero, size: screen), context: context) }
        } else {
            try draw(size: screen, to: maskURL) { context in
                context.setFillColor(gray: 0, alpha: 1)
                context.fill(CGRect(origin: .zero, size: screen))
            }
        }
        return Layout(size: size, screen: screenRect)
    }

    /// The composite is the bare device at this screen's size, so its margin is the bezel. Chrome
    /// shared by several sizes has no composite that fits, and says how wide its edges are instead.
    private static func bezelWidth(composite: PDF?, images: [String: Any], screen: CGSize) -> CGFloat {
        if let composite {
            let horizontal = (composite.size.width - screen.width) / 2
            let vertical = (composite.size.height - screen.height) / 2
            if horizontal > 0, abs(horizontal - vertical) < 1 { return horizontal }
        }
        let sizing = images["sizing"] as? [String: Any] ?? [:]
        return CGFloat((sizing["leftWidth"] as? NSNumber)?.doubleValue ?? 18)
    }

    private static func drawNineSlice(pdf: (String) -> PDF?, device: CGRect, in context: CGContext) {
        guard let topLeft = pdf("topLeft"), let topRight = pdf("topRight"), let bottomLeft = pdf("bottomLeft"),
            let bottomRight = pdf("bottomRight")
        else { return }
        let corner = topLeft.size
        context.setFillColor(gray: 0, alpha: 1)
        context.fill(device.insetBy(dx: corner.width / 2, dy: corner.height / 2))
        topLeft.draw(in: CGRect(origin: device.origin, size: corner), context: context)
        topRight.draw(in: CGRect(x: device.maxX - corner.width, y: device.minY, width: corner.width, height: corner.height), context: context)
        bottomLeft.draw(in: CGRect(x: device.minX, y: device.maxY - corner.height, width: corner.width, height: corner.height), context: context)
        bottomRight.draw(
            in: CGRect(x: device.maxX - corner.width, y: device.maxY - corner.height, width: corner.width, height: corner.height),
            context: context)
        let middleWidth = device.width - corner.width * 2
        let middleHeight = device.height - corner.height * 2
        pdf("top")?.draw(in: CGRect(x: device.minX + corner.width, y: device.minY, width: middleWidth, height: corner.height), context: context)
        pdf("bottom")?.draw(
            in: CGRect(x: device.minX + corner.width, y: device.maxY - corner.height, width: middleWidth, height: corner.height), context: context)
        pdf("left")?.draw(in: CGRect(x: device.minX, y: device.minY + corner.height, width: corner.width, height: middleHeight), context: context)
        pdf("right")?.draw(
            in: CGRect(x: device.maxX - corner.width, y: device.minY + corner.height, width: corner.width, height: middleHeight), context: context)
    }

    /// A side button sits against the device's left or right edge, `offsets.normal` from its top.
    private static func drawButton(_ button: [String: Any], art: URL, device: CGRect, in context: CGContext) {
        guard let name = button["image"] as? String, let image = PDF(art.appendingPathComponent("\(name).pdf")) else { return }
        let offsets = (button["offsets"] as? [String: Any])?["normal"] as? [String: Any] ?? [:]
        let dx = CGFloat((offsets["x"] as? NSNumber)?.doubleValue ?? 0)
        let dy = CGFloat((offsets["y"] as? NSNumber)?.doubleValue ?? 0)
        let x = button["anchor"] as? String == "right" ? device.maxX + dx : device.minX + dx - image.size.width
        image.draw(in: CGRect(x: x, y: device.minY + dy, width: image.size.width, height: image.size.height), context: context)
    }

    /// Draws into a transparent PNG `size` points across, with y running down the page.
    private static func draw(size: CGSize, to url: URL, _ body: (CGContext) -> Void) throws {
        let width = Int((size.width * scale).rounded(.up))
        let height = Int((size.height * scale).rounded(.up))
        guard
            let context = CGContext(
                data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
                space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
        else { throw Failure(reason: "chrome", message: "could not draw the device") }
        context.translateBy(x: 0, y: CGFloat(height))
        context.scaleBy(x: scale, y: -scale)
        body(context)
        guard let image = context.makeImage(),
            let destination = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil)
        else { throw Failure(reason: "chrome", message: "could not save the device") }
        CGImageDestinationAddImage(destination, image, nil)
        guard CGImageDestinationFinalize(destination) else { throw Failure(reason: "chrome", message: "could not save the device") }
    }

    private static func plist(_ url: URL) throws -> [String: Any] {
        try PropertyListSerialization.propertyList(from: Data(contentsOf: url), format: nil) as? [String: Any] ?? [:]
    }
}

/// The first page of a PDF from the device artwork.
private struct PDF {
    let page: CGPDFPage

    init?(_ url: URL) {
        guard let document = CGPDFDocument(url as CFURL), let page = document.page(at: 1) else { return nil }
        self.page = page
    }

    var size: CGSize { page.getBoxRect(.mediaBox).size }

    /// Stretches the page over `rect`, in a context whose y runs down.
    func draw(in rect: CGRect, context: CGContext) {
        let box = page.getBoxRect(.mediaBox)
        guard box.width > 0, box.height > 0 else { return }
        context.saveGState()
        context.translateBy(x: rect.minX, y: rect.maxY)
        context.scaleBy(x: rect.width / box.width, y: -rect.height / box.height)
        context.translateBy(x: -box.minX, y: -box.minY)
        context.drawPDFPage(page)
        context.restoreGState()
    }
}
