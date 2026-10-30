// swift-tools-version: 6.0
import PackageDescription

// FluidAudio 0.14.8 (Apache-2.0, github.com/FluidInference/FluidAudio) cut
// down to the Parakeet speech model the helper runs. Swift keeps every type a
// library declares, so the full package made the helper three times larger.
let package = Package(
    name: "FluidAudio",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "FluidAudio", targets: ["FluidAudio"])
    ],
    targets: [
        .target(
            name: "FluidAudio",
            dependencies: ["MachTaskSelfWrapper"],
            path: "Sources/FluidAudio"
        ),
        .target(
            name: "MachTaskSelfWrapper",
            path: "Sources/MachTaskSelfWrapper",
            publicHeadersPath: "include"
        ),
    ],
    cxxLanguageStandard: .cxx17
)
