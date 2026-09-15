use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn output(command: &mut Command) -> String {
    let result = command
        .output()
        .expect("Xcode command-line tools (including Swift) are required");
    assert!(
        result.status.success(),
        "native progress build failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout)
        .expect("tool output must be UTF-8")
        .trim()
        .to_owned()
}

fn main() {
    println!("cargo:rerun-if-changed=native");
    for name in [
        "DEVELOPER_DIR",
        "SDKROOT",
        "MACOSX_DEPLOYMENT_TARGET",
        "TOOLCHAINS",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        other => panic!("unsupported native progress architecture: {other}"),
    };
    let deployment = env::var("MACOSX_DEPLOYMENT_TARGET").unwrap_or_else(|_| "11.0".into());
    let target = format!("{arch}-apple-macosx{deployment}");
    let sdk = output(Command::new("xcrun").args(["--sdk", "macosx", "--show-sdk-path"]));
    let swift = output(Command::new("xcrun").args(["--find", "swiftc"]));
    let info: serde_json::Value = serde_json::from_str(&output(Command::new(&swift).args([
        "-print-target-info",
        "-target",
        &target,
    ])))
    .expect("Swift target info");
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let native = manifest.join("native");
    let mut sources = std::fs::read_dir(native.join("ThinkingOrbsKit/Sources/ThinkingOrbsKit"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "swift"))
        .collect::<Vec<_>>();
    sources.sort();
    sources.push(native.join("Progress.swift"));
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    output(
        Command::new(&swift)
            .args([
                "-emit-library",
                "-static",
                "-parse-as-library",
                "-whole-module-optimization",
                "-O",
                "-swift-version",
                "5",
                "-module-name",
                "SelaraProgress",
                "-target",
                &target,
                "-sdk",
                &sdk,
            ])
            .args(sources)
            .arg("-o")
            .arg(out.join("libselara_progress.a")),
    );
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=selara_progress");
    for path in info["paths"]["runtimeLibraryPaths"]
        .as_array()
        .expect("Swift runtime library paths")
    {
        println!("cargo:rustc-link-search=native={}", path.as_str().unwrap());
    }
    // Compatibility archives contain back-deployment shims, not redistributable
    // toolchain dylibs. Swift object autolink directives select the needed libs.
    if let Some(resource) = info["paths"]["runtimeResourcePath"].as_str() {
        let compatibility = Path::new(resource).join("macosx");
        println!("cargo:rustc-link-search=native={}", compatibility.display());
    }
    println!("cargo:rustc-link-lib=framework=AppKit");
    println!("cargo:rustc-link-lib=framework=SwiftUI");
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
}
