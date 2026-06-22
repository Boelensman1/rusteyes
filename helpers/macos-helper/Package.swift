// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "rusteyes-macos-helper",
    platforms: [
        .macOS(.v13)
    ],
    products: [
        .executable(
            name: "rusteyes-macos-helper",
            targets: ["RustEyesMacOSHelper"]
        )
    ],
    targets: [
        .executableTarget(
            name: "RustEyesMacOSHelper",
            path: "Sources/RustEyesMacOSHelper"
        )
    ]
)
