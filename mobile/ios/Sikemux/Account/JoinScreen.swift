import SwiftUI

struct JoinScreen: View {
    let core: String
    let name: String
    var arrived = false

    var body: some View {
        Screen {
            Nav("Join")
            Spacer()
        }
    }
}
