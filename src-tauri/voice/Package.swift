// swift-tools-version:5.10
import PackageDescription

let package = Package(
    name: "sikemux-voice",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "sikemux-voice", targets: ["SikemuxVoice"])
    ],
    dependencies: [
        .package(url: "https://github.com/FluidInference/FluidAudio.git", exact: "0.14.8")
    ],
    targets: [
        .executableTarget(
            name: "SikemuxVoice",
            dependencies: [.product(name: "FluidAudio", package: "FluidAudio")],
            path: "Sources/SikemuxVoice"
        )
    ]
)
