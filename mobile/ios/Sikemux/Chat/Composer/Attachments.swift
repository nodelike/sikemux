import SwiftUI

/// What the next message carries, in a row over the composer's field: links and text from the stash first, then
/// photos as thumbnails and files by name. A file that did not reach the host is edged red; a tap tries again.
struct ComposerAttachments<Lead: View>: View {
    @Environment(\.palette) private var palette
    let attachments: [Attachment]
    /// The host a picture only it has is read from.
    var core: String?
    let onRemove: (String) -> Void
    let onRetry: () -> Void
    @ViewBuilder var lead: Lead

    var body: some View {
        let failed = attachments.filter { $0.upload == .failed }
        VStack(alignment: .leading, spacing: 0) {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 7) {
                    lead
                    ForEach(attachments) { attachment in tile(attachment) }
                }
                .padding(.horizontal, 4)
                .padding(.top, 4)
                .padding(.bottom, 2)
            }
            if let first = failed.first {
                Text(
                    "\(failed.count == 1 ? "\(first.name) did not reach the host" : "\(failed.count) files did not reach the host")\(first.problem.map { ": \($0)" } ?? ""). Tap to try again."
                )
                .font(Theme.ui(12))
                .lineSpacing(16 - 12 * 1.2)
                .foregroundStyle(palette.danger)
                .lineLimit(2)
                .padding(.horizontal, 6)
                .padding(.top, 6)
            }
        }
    }

    private func tile(_ attachment: Attachment) -> some View {
        let sending = attachment.upload == .sending
        let broken = attachment.upload == .failed
        let shape = RoundedRectangle(cornerRadius: 9, style: .continuous)
        return Group {
            if attachment.kind == .image {
                TileImage(attachment: attachment, core: core).frame(width: 58)
            } else {
                FileChip(name: attachment.name, size: attachment.size == 0 ? nil : attachment.size, tall: true)
                    .frame(maxWidth: 190)
            }
        }
        .frame(height: 44)
        .background(palette.sunken)
        .clipShape(shape)
        .overlay(shape.strokeBorder(broken ? palette.danger : palette.border, lineWidth: 1))
        .overlay {
            if sending || broken {
                ZStack {
                    Color(hex: "rgba(9, 9, 11, 0.55)")
                    if sending {
                        ProgressView().tint(palette.ink)
                    } else {
                        IconView(icon: Icons.refresh, size: 18).foregroundStyle(palette.ink)
                    }
                }
                .clipShape(shape)
            }
        }
        .overlay(alignment: .topTrailing) {
            if !sending && !broken {
                Button {
                    onRemove(attachment.id)
                } label: {
                    IconView(icon: Icons.close, size: 10)
                        .foregroundStyle(palette.ground)
                        .frame(width: 18, height: 18)
                        .background(Circle().fill(palette.ink))
                        .padding(3)
                        .contentShape(Rectangle().inset(by: -6))
                }
                .buttonStyle(PressFade())
                .accessibilityLabel("Remove \(attachment.name)")
            }
        }
        .contentShape(shape)
        .onTapGesture { if broken { onRetry() } }
        .accessibilityAddTraits(broken ? .isButton : [])
        .accessibilityLabel(
            broken
                ? "\(attachment.name) did not reach the host\(attachment.problem.map { ": \($0)" } ?? ""). Try again"
                : "\(attachment.name)\(sending ? ", sending" : "")")
    }
}

extension ComposerAttachments where Lead == EmptyView {
    init(
        attachments: [Attachment], core: String? = nil, onRemove: @escaping (String) -> Void,
        onRetry: @escaping () -> Void
    ) {
        self.init(attachments: attachments, core: core, onRemove: onRemove, onRetry: onRetry) { EmptyView() }
    }
}

/// A picture on the composer: this phone's copy, or one fetched from the host for a picture only the host has.
private struct TileImage: View {
    @Environment(\.palette) private var palette
    let attachment: Attachment
    let core: String?
    @State private var image: UIImage?

    var body: some View {
        Group {
            if let image {
                Image(uiImage: image).resizable().scaledToFill()
            } else {
                IconView(icon: Icons.image, size: 16).foregroundStyle(palette.inkDim)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .task(id: attachment.uri) {
            var url: URL? = attachment.uri
            if attachment.onHostOnly, let path = attachment.path {
                url = if let core { await HostFiles.shared.read(core: core, path: path) } else { nil }
            }
            guard let url else { return }
            image = await Task.detached { Self.thumbnail(url) }.value
        }
    }

    /// Enough pixels for its 58 points at 3x, and a little more.
    nonisolated private static func thumbnail(_ url: URL) -> UIImage? {
        guard let source = CGImageSourceCreateWithURL(url as CFURL, nil) else { return nil }
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceThumbnailMaxPixelSize: 240,
            kCGImageSourceCreateThumbnailWithTransform: true,
        ]
        return CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary).map { UIImage(cgImage: $0) }
    }
}

/// A file by its name and size.
struct FileChip: View {
    @Environment(\.palette) private var palette
    let name: String
    var size: Int?
    var image = false
    /// Fills a composer tile, rather than standing on its own.
    var tall = false

    var body: some View {
        HStack(spacing: 8) {
            IconView(icon: image ? Icons.image : Icons.file, size: 16).foregroundStyle(palette.inkDim)
            VStack(alignment: .leading, spacing: 2) {
                Text(name).font(Theme.mono(12)).foregroundStyle(palette.ink).lineLimit(1).truncationMode(.middle)
                if let size { Text(Uploads.sizeLabel(size)).font(Theme.ui(11)).foregroundStyle(palette.inkFaint) }
            }
        }
        .padding(.vertical, 7)
        .padding(.leading, 10)
        .padding(.trailing, tall ? 26 : 10)
        .frame(maxWidth: tall ? nil : 220, maxHeight: tall ? .infinity : nil, alignment: .leading)
        .background {
            if !tall {
                RoundedRectangle(cornerRadius: 9, style: .continuous).fill(palette.raised)
                    .overlay(
                        RoundedRectangle(cornerRadius: 9, style: .continuous).strokeBorder(palette.border, lineWidth: 1)
                    )
            }
        }
    }
}
