// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "parakeet-coreml",
    platforms: [
        .macOS(.v14),
    ],
    products: [
        .executable(
            name: "parakeet-coreml",
            targets: ["parakeet-coreml"]
        ),
    ],
    dependencies: [
        .package(url: "https://github.com/FluidInference/FluidAudio.git", branch: "main"),
    ],
    targets: [
        .executableTarget(
            name: "parakeet-coreml",
            dependencies: [
                .product(name: "FluidAudio", package: "FluidAudio"),
            ],
            path: "Sources"
        ),
    ]
)
