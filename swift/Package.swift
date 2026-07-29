// swift-tools-version:5.9
import PackageDescription

// The Swift frontend consumes the same C ABI as the Python and Julia bindings.
// `liblilook_ffi` is built by cargo; for iOS this becomes an XCFramework built
// for aarch64-apple-ios and the simulator triple.
let package = Package(
    name: "Lilook",
    platforms: [.macOS(.v13), .iOS(.v16)],
    products: [
        .library(name: "Lilook", targets: ["Lilook"])
    ],
    targets: [
        .systemLibrary(name: "CLilook", path: "Sources/CLilook"),
        .target(
            name: "Lilook",
            dependencies: ["CLilook"],
            linkerSettings: [.linkedLibrary("lilook_ffi")]
        ),
        .testTarget(name: "LilookTests", dependencies: ["Lilook"]),
    ]
)
