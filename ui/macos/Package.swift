// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "Classick",
    platforms: [.macOS(.v15)],
    targets: [
        .executableTarget(
            name: "Classick",
            path: "Sources/Classick",
            // Required so `@main struct ...: App` is treated as the entry point
            // rather than script-mode top-level code in a single-file target.
            swiftSettings: [.unsafeFlags(["-parse-as-library"])]
        ),
        .testTarget(
            name: "ClassickTests",
            dependencies: ["Classick"],
            path: "Tests/ClassickTests"
        ),
    ]
)
