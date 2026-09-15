fn main() {
    // Packaging stages this file before Cargo runs. Direct development builds
    // may not have staged runtime metadata yet; omit the version in that case.
    let provenance_path = "runtime-notices/selara-codex.provenance.json";
    println!("cargo:rerun-if-changed={provenance_path}");
    let provenance = match std::fs::read_to_string(provenance_path) {
        Ok(contents) => Some(contents),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => panic!("could not read bundled runtime provenance: {error}"),
    };
    let out_dir = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR is set"));
    std::fs::write(
        out_dir.join("bundled_codex_provenance.rs"),
        format!("{provenance:?}"),
    )
    .expect("could not write bundled runtime provenance");
    tauri_build::build()
}
