// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "SelaraProgressPreview",
    platforms: [.macOS(.v13)],
    products: [.executable(name: "selara-progress-preview", targets: ["NativeProgressPreview"])],
    dependencies: [.package(path: "../../apps/selara/native/ThinkingOrbsKit")],
    targets: [.executableTarget(name: "NativeProgressPreview", dependencies: ["ThinkingOrbsKit"])]
)
