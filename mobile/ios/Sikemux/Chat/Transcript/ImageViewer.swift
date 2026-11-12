import SwiftUI
import UIKit

/// A picture over the whole screen, pinched to zoom, with Save through the share sheet. A picture still on its way from
/// the host shows a spinner, or why it could not come.
struct ImageViewer: View {
    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss
    let picture: ShownPicture
    @State private var saving = false

    var body: some View {
        let name = picture.name
        ZStack(alignment: .top) {
            Color.black.opacity(0.94).ignoresSafeArea()
            if let image = picture.image {
                ZoomingImage(image: image).ignoresSafeArea().accessibilityLabel(name)
            } else if picture.problem == nil {
                ProgressView().tint(palette.ink).frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            HStack(spacing: 12) {
                control(Icons.close, label: String(localized: "Close")) { dismiss() }
                Text(name).font(Theme.mono(12)).foregroundStyle(palette.inkDim).lineLimit(1).truncationMode(.middle)
                    .frame(maxWidth: .infinity)
                control(Icons.download, label: String(localized: "Save \(name)")) { saving = true }
                    .disabled(picture.image == nil)
                    .opacity(picture.image == nil ? 0.5 : 1)
            }
            .padding(.horizontal, 16)
            .padding(.top, 8)
            if let problem = picture.problem {
                ProblemLine(text: problem, centred: true)
                    .padding(.horizontal, 16)
                    .frame(maxHeight: .infinity, alignment: .bottom)
                    .padding(.bottom, 16)
            }
        }
        .presentationBackground(.clear)
        .sheet(isPresented: $saving) {
            if let image = picture.image { ShareSheet(items: [image]).presentationDetents([.medium, .large]) }
        }
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
