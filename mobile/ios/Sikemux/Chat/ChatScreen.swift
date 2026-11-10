import SwiftUI

struct ChatScreen: View {
    let core: String
    let agentId: String
    var started = false

    var body: some View {
        Screen {
            Nav("Chat")
            Spacer()
        }
    }
}
