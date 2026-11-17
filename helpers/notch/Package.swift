// swift-tools-version:5.10
import PackageDescription

let package = Package(
    name: "sikemux-notch",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "sikemux-notch", targets: ["SikemuxNotch"])
    ],
    targets: [
        .target(
            name: "NotchKit",
            dependencies: ["Sentry"],
            path: "Sources/NotchKit",
            linkerSettings: [.linkedLibrary("c++"), .linkedLibrary("z")]
        ),
        .binaryTarget(
            name: "Sentry",
            url: "https://github.com/getsentry/sentry-cocoa/releases/download/9.33.0/Sentry.xcframework.zip",
            checksum: "ede8d9a33a43f70d33e2f24752a681dc9dcb4a7b6245bd6bca2fa7b30e2c63f5"
        ),
        .executableTarget(name: "SikemuxNotch", dependencies: ["NotchKit"], path: "Sources/SikemuxNotch"),
        // Renders every state of the island to PNGs: `swift run notch-snapshots <dir>`.
        .executableTarget(name: "notch-snapshots", dependencies: ["NotchKit"], path: "Sources/Snapshots"),
    ]
)
