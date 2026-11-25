// swift-tools-version:5.10
import PackageDescription

let package = Package(
    name: "sikemux-voice",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "sikemux-voice", targets: ["SikemuxVoice"])
    ],
    dependencies: [
        .package(path: "FluidAudio")
    ],
    targets: [
        .target(
            name: "AudioRing",
            path: "Sources/AudioRing",
            publicHeadersPath: "include"
        ),
        .executableTarget(
            name: "SikemuxVoice",
            dependencies: [.product(name: "FluidAudio", package: "FluidAudio"), "AudioRing"],
            path: "Sources/SikemuxVoice"
        ),
    ]
)
