import OSLog
import SwiftUI

private let log = Logger(subsystem: "com.nodelike.sikemux.simfixture", category: "fixture")

@main
struct SimFixtureApp: App {
    var body: some Scene {
        WindowGroup { ContentView() }
    }
}

struct ContentView: View {
    @State private var count = 0
    @State private var text = ""
    @State private var zoom: CGFloat = 1

    var body: some View {
        VStack(spacing: 16) {
            Text("Count: \(count)").accessibilityIdentifier("count")
            Button("Add one") {
                count += 1
                log.notice("tapped add one, count \(count)")
            }
            .accessibilityIdentifier("add")
            TextField("Type here", text: $text)
                .textFieldStyle(.roundedBorder)
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
                .accessibilityIdentifier("field")
            Text("Echo: \(text)").accessibilityIdentifier("echo")
            Text("Zoom: \(String(format: "%.1f", zoom))")
                .frame(maxWidth: .infinity, minHeight: 120)
                .background(Color.blue.opacity(0.15))
                .accessibilityIdentifier("zoom")
                .gesture(MagnifyGesture().onEnded { value in
                    zoom = (zoom * value.magnification * 10).rounded() / 10
                    log.notice("pinched to zoom \(zoom)")
                })
            List(1...60, id: \.self) { row in
                Text("Row \(row)").accessibilityIdentifier("row-\(row)")
            }
        }
        .padding()
    }
}
