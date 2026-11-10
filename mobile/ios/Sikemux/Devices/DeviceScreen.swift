import SwiftUI

struct DeviceScreen: View {
    let core: String
    var linkedTab: Route.DeviceTab?

    var body: some View {
        Screen {
            Nav("Host")
            Spacer()
        }
    }
}
