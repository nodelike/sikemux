import SwiftUI
import UIKit

/// A picture over the whole screen, pinched to zoom, with Save through the share sheet.
struct ImageViewer: View {
    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss
    let image: UIImage
    let name: String
    @State private var saving = false

    var body: some View {
        ZStack(alignment: .top) {
            Color.black.opacity(0.94).ignoresSafeArea()
            ZoomingImage(image: image).ignoresSafeArea().accessibilityLabel(name)
            HStack(spacing: 12) {
                control(Icons.close, label: "Close") { dismiss() }
                Text(name).font(Theme.mono(12)).foregroundStyle(palette.inkDim).lineLimit(1).truncationMode(.middle)
                    .frame(maxWidth: .infinity)
                control(Icons.download, label: "Save \(name)") { saving = true }
            }
            .padding(.horizontal, 16)
            .padding(.top, 8)
        }
        .presentationBackground(.clear)
        .sheet(isPresented: $saving) { ShareSheet(items: [image]).presentationDetents([.medium, .large]) }
    }

    private func control(_ icon: IconDef, label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            IconView(icon: icon, size: 16)
                .foregroundStyle(palette.ink)
                .frame(width: 36, height: 36)
                .background(RoundedRectangle(cornerRadius: Radius.control).fill(palette.raised))
                .overlay(RoundedRectangle(cornerRadius: Radius.control).strokeBorder(palette.border, lineWidth: 1))
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }
}

private struct ZoomingImage: UIViewRepresentable {
    let image: UIImage

    func makeUIView(context: Context) -> UIScrollView {
        let scroll = UIScrollView()
        scroll.minimumZoomScale = 1
        scroll.maximumZoomScale = 5
        scroll.showsVerticalScrollIndicator = false
        scroll.showsHorizontalScrollIndicator = false
        scroll.delegate = context.coordinator
        let view = UIImageView(image: image)
        view.contentMode = .scaleAspectFit
        view.frame = scroll.bounds
        view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        scroll.addSubview(view)
        context.coordinator.view = view
        return scroll
    }

    func updateUIView(_ view: UIScrollView, context: Context) {}

    func makeCoordinator() -> Coordinator { Coordinator() }

    final class Coordinator: NSObject, UIScrollViewDelegate {
        var view: UIView?
        func viewForZooming(in scrollView: UIScrollView) -> UIView? { view }
    }
}

struct ShareSheet: UIViewControllerRepresentable {
    let items: [Any]

    func makeUIViewController(context: Context) -> UIActivityViewController {
        UIActivityViewController(activityItems: items, applicationActivities: nil)
    }

    func updateUIViewController(_ controller: UIActivityViewController, context: Context) {}
}
