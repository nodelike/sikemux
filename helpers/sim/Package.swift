// swift-tools-version:6.0
import PackageDescription

// The parts of facebook/idb (MIT) the helper drives the iOS Simulator with,
// copied in under idb/ from commit 45e96c070ce62ee1df14ba21ccd8c07b720a8684. FBControlCore's Swift and Objective-C halves import
// each other, which one SwiftPM target cannot hold, so build-sim-helper.mjs
// builds it with XcodeGen into Frameworks/FBControlCore.xcframework first.
let root = Context.packageDirectory
let developerDir = Context.environment["DEVELOPER_DIR"] ?? "/Applications/Xcode.app/Contents/Developer"
let privateHeaders = "\(root)/idb/PrivateHeaders"
let privateModules = [
    "AXRuntime", "CoreSimulatorUtilities", "DTXConnectionServices", "SimulatorKit",
    "AccessibilityPlatformTranslation", "CoreSimDeviceIO", "CoreSimulator", "SimulatorApp",
]
let simulatorFrameworks: SwiftSetting = .unsafeFlags(
    ["-F", "\(developerDir)/Library/PrivateFrameworks", "-Xcc", "-I\(privateHeaders)"]
        + privateModules.flatMap { ["-Xcc", "-fmodule-map-file=\(privateHeaders)/\($0)/module.modulemap"] }
)

/// idb's own code is kept as upstream wrote it, so its warnings are not ours to act on.
let upstream: SwiftSetting = .unsafeFlags(["-suppress-warnings"])

let package = Package(
    name: "sikemux-sim",
    platforms: [.macOS(.v15)],
    products: [
        .executable(name: "sikemux-sim", targets: ["SikemuxSim"])
    ],
    targets: [
        .binaryTarget(name: "FBControlCore", path: "Frameworks/FBControlCore.xcframework"),
        .target(name: "CompanionUtilities", path: "idb/CompanionUtilities", swiftSettings: [upstream]),
        .target(name: "SimulatorIPC", path: "idb/SimulatorIPC", swiftSettings: [upstream]),
        .target(name: "SimulatorFrameworkBridgeProtocol", path: "idb/SimulatorFrameworkBridgeProtocol", swiftSettings: [upstream]),
        .target(
            name: "FBSimulatorControl",
            dependencies: ["FBControlCore", "CompanionUtilities", "SimulatorIPC", "SimulatorFrameworkBridgeProtocol"],
            path: "idb/FBSimulatorControl",
            exclude: ["FBSimulatorControl-Info.plist", "FBSimulatorControl.xcconfig", "FBSimulatorControl.h", "README.md"],
            swiftSettings: [simulatorFrameworks, upstream]
        ),
        .executableTarget(
            name: "SikemuxSim",
            dependencies: ["FBSimulatorControl", "FBControlCore"],
            path: "Sources/SikemuxSim",
            swiftSettings: [simulatorFrameworks, .swiftLanguageMode(.v5)],
            linkerSettings: [
                // CoreSimulator comes with Xcode and is loaded only if it is there, so the
                // helper starts on a Mac without Xcode and can say what is missing.
                .unsafeFlags(
                    ["CoreSimulator", "AccessibilityPlatformTranslation"].flatMap {
                        ["-Xlinker", "-weak_library", "-Xlinker", "\(privateHeaders)/\($0)/\($0).tbd"]
                    }
                        // FBControlCore adds Objective-C categories that nothing references by name.
                        + ["-Xlinker", "-force_load", "-Xlinker", "\(root)/Frameworks/FBControlCore.a"]
                )
            ]
        ),
        .testTarget(
            name: "SikemuxSimTests",
            dependencies: ["SikemuxSim"],
            path: "Tests/SikemuxSimTests",
            swiftSettings: [simulatorFrameworks]
        ),
    ]
)
