// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "resteyes-macos-helper",
    platforms: [
        .macOS(.v13)
    ],
    products: [
        .executable(
            name: "resteyes-macos-helper",
            targets: ["ResteyesMacOSHelper"]
        )
    ],
    targets: [
        .executableTarget(
            name: "ResteyesMacOSHelper",
            path: "Sources/ResteyesMacOSHelper"
        )
    ]
)
