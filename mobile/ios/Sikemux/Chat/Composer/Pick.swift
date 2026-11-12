import AVFoundation
import Photos
import PhotosUI
import SwiftUI
import UIKit
import UniformTypeIdentifiers

/// Where a message's photos and files come from on this phone: the system's photo picker, the camera, the Files app
/// or the app's own gallery. Photos go as JPEG no larger than 2048 points on their longest side, which agents read at
/// full detail.
@MainActor
enum Pick {
    private static let longestEdge: CGFloat = 2048
    private static var picks = 0
    /// Keeps the delegate of the system screen on show alive until it answers.
    private static var showing: NSObject?

    private static func nextId() -> String {
        picks += 1
        return "pick-\(picks)"
    }

    /// Picked in the system's photo picker, which needs no access to the library.
    static func photos(limit: Int) async throws -> [Attachment] {
        var config = PHPickerConfiguration()
        config.filter = .images
        config.selectionLimit = limit
        config.preferredAssetRepresentationMode = .compatible
        let results: [PHPickerResult] = await withCheckedContinuation { done in
            let picker = PHPickerViewController(configuration: config)
            let delegate = PhotoDelegate { done.resume(returning: $0) }
            picker.delegate = delegate
            showing = delegate
            present(picker)
        }
        showing = nil
        var photos: [Attachment] = []
        for result in results.prefix(limit) {
            guard let image = try await load(result.itemProvider) else { continue }
            photos.append(try jpeg(image, name: result.itemProvider.suggestedName))
        }
        return photos
    }

    /// Photos chosen in the app's own gallery, in the order they were chosen.
    static func library(_ chosen: [PHAsset]) async throws -> [Attachment] {
        var photos: [Attachment] = []
        for asset in chosen {
            let options = PHImageRequestOptions()
            options.isNetworkAccessAllowed = true
            options.deliveryMode = .highQualityFormat
            options.version = .current
            let data: Data? = await withCheckedContinuation { done in
                PHImageManager.default().requestImageDataAndOrientation(for: asset, options: options) { data, _, _, _ in
                    done.resume(returning: data)
                }
            }
            guard let data, let image = UIImage(data: data) else { continue }
            let name = PHAssetResource.assetResources(for: asset).first?.originalFilename
            photos.append(try jpeg(image, name: name))
        }
        return photos
    }

    static func camera() async throws -> [Attachment] {
        guard await AVCaptureDevice.requestAccess(for: .video) else {
            throw ChatProblem(String(localized: "Sikemux has no access to the camera; allow it in Settings"))
        }
        guard UIImagePickerController.isSourceTypeAvailable(.camera) else { return [] }
        let image: UIImage? = await withCheckedContinuation { done in
            let picker = UIImagePickerController()
            picker.sourceType = .camera
            picker.mediaTypes = [UTType.image.identifier]
            let delegate = CameraDelegate { done.resume(returning: $0) }
            picker.delegate = delegate
            showing = delegate
            present(picker)
        }
        showing = nil
        guard let image else { return [] }
        return [try jpeg(image, name: nil)]
    }

    static func files(limit: Int) async throws -> [Attachment] {
        let urls: [URL] = await withCheckedContinuation { done in
            let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.item], asCopy: true)
            picker.allowsMultipleSelection = true
            let delegate = FileDelegate { done.resume(returning: $0) }
            picker.delegate = delegate
            showing = delegate
            present(picker)
        }
        showing = nil
        return urls.prefix(limit).map(file)
    }

    /// A file already on this phone, as the next message carries it.
    static func file(_ url: URL) -> Attachment {
        let type = UTType(filenameExtension: url.pathExtension)
        let mime = type?.preferredMIMEType ?? "application/octet-stream"
        let size = (try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0
        let image = type.map { $0.conforms(to: .image) && !$0.conforms(to: .svg) } ?? false
        return Attachment(
            id: nextId(), kind: image ? .image : .file, name: url.lastPathComponent, mime: mime, uri: url, size: size)
    }

    /// Scaled down and as JPEG, which every agent reads (HEIC included).
    private static func jpeg(_ image: UIImage, name: String?) throws -> Attachment {
        let size = image.size
        let scale = min(1, longestEdge / max(size.width, size.height))
        let target = CGSize(width: (size.width * scale).rounded(), height: (size.height * scale).rounded())
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        let drawn = UIGraphicsImageRenderer(size: target, format: format).image { _ in
            image.draw(in: CGRect(origin: .zero, size: target))
        }
        guard let data = drawn.jpegData(compressionQuality: 0.85) else {
            throw ChatProblem(String(localized: "could not read the photo"))
        }
        let stem = name.map { ($0 as NSString).deletingPathExtension }.flatMap { $0.isEmpty ? nil : $0 } ?? "photo"
        let url = FileManager.default.temporaryDirectory.appending(path: "\(UUID().uuidString).jpg")
        try data.write(to: url)
        return Attachment(
            id: nextId(), kind: .image, name: "\(stem).jpg", mime: "image/jpeg", uri: url, size: data.count)
    }

    private static func load(_ provider: NSItemProvider) async throws -> UIImage? {
        guard provider.canLoadObject(ofClass: UIImage.self) else { return nil }
        return try await withCheckedThrowingContinuation { done in
            provider.loadObject(ofClass: UIImage.self) { image, error in
                if let error { done.resume(throwing: error) } else { done.resume(returning: image as? UIImage) }
            }
        }
    }

    private static func present(_ controller: UIViewController) {
        let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
        var top = scenes.flatMap(\.windows).first(where: \.isKeyWindow)?.rootViewController
        while let shown = top?.presentedViewController { top = shown }
        top?.present(controller, animated: true)
    }
}

private final class PhotoDelegate: NSObject, PHPickerViewControllerDelegate {
    let done: (sending [PHPickerResult]) -> Void
    init(_ done: @escaping (sending [PHPickerResult]) -> Void) { self.done = done }

    func picker(_ picker: PHPickerViewController, didFinishPicking results: [PHPickerResult]) {
        picker.dismiss(animated: true)
        // Only the main actor reads them: the picker answers there, and `photos` waits there.
        nonisolated(unsafe) let picked = results
        done(picked)
    }
}

private final class CameraDelegate: NSObject, UIImagePickerControllerDelegate, UINavigationControllerDelegate {
    let done: (UIImage?) -> Void
    init(_ done: @escaping (UIImage?) -> Void) { self.done = done }

    func imagePickerController(
        _ picker: UIImagePickerController, didFinishPickingMediaWithInfo info: [UIImagePickerController.InfoKey: Any]
    ) {
        picker.dismiss(animated: true)
        done(info[.originalImage] as? UIImage)
    }

    func imagePickerControllerDidCancel(_ picker: UIImagePickerController) {
        picker.dismiss(animated: true)
        done(nil)
    }
}

private final class FileDelegate: NSObject, UIDocumentPickerDelegate {
    let done: ([URL]) -> Void
    init(_ done: @escaping ([URL]) -> Void) { self.done = done }

    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) { done(urls) }
    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) { done([]) }
}

/// The newest photos on the phone, a page at a time, while the attach sheet is open. Access is asked for the first
/// time it opens, never before.
@MainActor @Observable
final class PhotoLibrary: NSObject, PHPhotoLibraryChangeObserver {
    enum Access { case asking, all, limited, denied }

    private(set) var access: Access = .asking
    private(set) var photos: [PHAsset] = []
    @ObservationIgnored private var fetched: PHFetchResult<PHAsset>?
    @ObservationIgnored private var watching = false
    /// A multiple of the grid's four columns, so a page never leaves a row half full.
    private static let page = 96

    var sees: Bool { access == .all || access == .limited }
    /// The phone shows its prompt only before the person has answered it.
    var canAsk: Bool { PHPhotoLibrary.authorizationStatus(for: .readWrite) == .notDetermined }

    /// Shows the photos the person already let the app see, without asking.
    func open() {
        take(PHPhotoLibrary.authorizationStatus(for: .readWrite))
    }

    /// Shows the system's prompt for the photos, once the person asks for them.
    func ask() {
        Task { take(await PHPhotoLibrary.requestAuthorization(for: .readWrite)) }
    }

    private func take(_ status: PHAuthorizationStatus) {
        access =
            switch status {
            case .authorized: .all
            case .limited: .limited
            case .notDetermined: .asking
            default: .denied
            }
        if sees { reload() }
    }

    func close() {
        if photos.count > Self.page { photos = Array(photos.prefix(Self.page)) }
    }

    func more() {
        guard let fetched, photos.count < fetched.count else { return }
        let end = min(fetched.count, photos.count + Self.page)
        photos += fetched.objects(at: IndexSet(photos.count..<end))
    }

    func manage() {
        let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
        var top = scenes.flatMap(\.windows).first(where: \.isKeyWindow)?.rootViewController
        while let shown = top?.presentedViewController { top = shown }
        if let top { PHPhotoLibrary.shared().presentLimitedLibraryPicker(from: top) }
    }

    private func reload() {
        if !watching {
            watching = true
            PHPhotoLibrary.shared().register(self)
        }
        let options = PHFetchOptions()
        options.sortDescriptors = [NSSortDescriptor(key: "creationDate", ascending: false)]
        let result = PHAsset.fetchAssets(with: .image, options: options)
        fetched = result
        photos = result.objects(at: IndexSet(0..<min(result.count, Self.page)))
    }

    nonisolated func photoLibraryDidChange(_ change: PHChange) {
        Task { @MainActor in if self.sees { self.reload() } }
    }
}
