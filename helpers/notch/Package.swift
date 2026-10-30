// swift-tools-version:5.10
import PackageDescription

let package = Package(
    name: "sikemux-notch",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "sikemux-notch", targets: ["SikemuxNotch"])
    ],
    targets: [
        .target(name: "NotchKit", path: "Sources/NotchKit"),
        .executableTarget(name: "SikemuxNotch", dependencies: ["NotchKit"], path: "Sources/SikemuxNotch"),
        // Renders every state of the island to PNGs: `swift run notch-snapshots <dir>`.
        .executableTarget(name: "notch-snapshots", dependencies: ["NotchKit"], path: "Sources/Snapshots"),
    ]
)
