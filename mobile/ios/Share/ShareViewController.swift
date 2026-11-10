import SwiftUI
import UIKit

/// Sikemux in the share menu: the sheet that puts what was shared in a chat's draft or the stash.
final class ShareViewController: UIViewController {
  private let model = ShareModel()

  override func viewDidLoad() {
    super.viewDidLoad()
    Theme.registerFonts()
    overrideUserInterfaceStyle = .dark
    view.backgroundColor = UIColor(palette.overlay)
    model.onFinish = { [weak self] in self?.finish() }
    let sheet = UIHostingController(rootView: ShareView(model: model, cancel: { [weak self] in self?.cancel() }))
    sheet.view.backgroundColor = .clear
    addChild(sheet)
    sheet.view.frame = view.bounds
    sheet.view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
    view.addSubview(sheet.view)
    sheet.didMove(toParent: self)
    let inputs = extensionContext?.inputItems.compactMap { $0 as? NSExtensionItem } ?? []
    Task { await model.load(inputs) }
  }

  private func finish() {
    model.cleanUp()
    extensionContext?.completeRequest(returningItems: nil)
  }

  private func cancel() {
    model.cleanUp()
    extensionContext?.cancelRequest(withError: NSError(domain: NSCocoaErrorDomain, code: NSUserCancelledError))
  }
}
