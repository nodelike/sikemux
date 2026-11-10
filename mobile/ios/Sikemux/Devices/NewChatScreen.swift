import SwiftUI

struct NewChatScreen: View {
    let core: String
    var project: String?
    var launcher: String?

    var body: some View {
        Screen {
            Nav("New chat")
            Spacer()
        }
    }
}
