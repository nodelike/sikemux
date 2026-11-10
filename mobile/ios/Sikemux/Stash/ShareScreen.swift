import SwiftUI

struct ShareScreen: View {
    let id: String
    let key: String?

    var body: some View {
        Screen {
            Nav("Shared chat")
            Spacer()
        }
    }
}
