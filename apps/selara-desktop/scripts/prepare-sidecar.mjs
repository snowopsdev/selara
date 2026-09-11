// Build the `selara` CLI in release mode and stage it as a Tauri sidecar.
//
// Tauri 2 looks for sidecars at `src-tauri/binaries/<name>-<target-triple>`
// (declared under `bundle.externalBin` in tauri.conf.json). `tauri-build`
// copies the file next to the app binary, so the Settings app can spawn
// `selara serve` with `app.shell().sidecar("selara")`. Runs before
// `tauri dev` and `tauri build` via the beforeDev/BuildCommand hooks.
import { execFileSync } from "node:child_process";
import { chmodSync, copyFileSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "..", "..", "..");
const binariesDir = resolve(here, "..", "src-tauri", "binaries");

function run(cmd, args, opts = {}) {
  return execFileSync(cmd, args, {
    cwd: repoRoot,
    stdio: ["ignore", "pipe", "inherit"],
    encoding: "utf8",
    ...opts,
  });
}

// Host triple, e.g. aarch64-apple-darwin. `--target <triple>` overrides it
// for cross builds and makes cargo build into target/<triple>/release.
const targetIdx = process.argv.indexOf("--target");
const triple = targetIdx !== -1
  ? process.argv[targetIdx + 1]
  : (run("rustc", ["-vV"]).match(/^host:\s*(\S+)/m) || [])[1];
if (!triple) {
  console.error("prepare-sidecar: could not read the host triple from `rustc -vV`");
  process.exit(1);
}

const cargoArgs = ["build", "-p", "selara", "--release"];
if (triple !== "aarch64-apple-darwin") throw new Error("The bundled writing runtime currently supports macOS Apple Silicon only");
run("sh", [join(repoRoot, "scripts/codex-runtime/build.sh")], { stdio: "inherit" });
if (targetIdx !== -1) cargoArgs.push("--target", triple);
console.log(`prepare-sidecar: cargo ${cargoArgs.join(" ")}`);
run("cargo", cargoArgs, { stdio: "inherit" });

const exe = process.platform === "win32" ? ".exe" : "";
const built = targetIdx !== -1
  ? join(repoRoot, "target", triple, "release", `selara${exe}`)
  : join(repoRoot, "target", "release", `selara${exe}`);
const dest = join(binariesDir, `selara-${triple}${exe}`);
mkdirSync(binariesDir, { recursive: true });
copyFileSync(built, dest);
if (process.platform !== "win32") chmodSync(dest, 0o755);
console.log(`prepare-sidecar: ${built} -> ${dest}`);
const runtime = join(binariesDir, `selara-codex-${triple}`);
copyFileSync(join(repoRoot, "target/selara-codex"), runtime);
chmodSync(runtime, 0o755);
const notices = join(here, "..", "src-tauri", "runtime-notices");
mkdirSync(notices, { recursive: true });
for (const suffix of ["LICENSE", "NOTICE", "provenance.json"]) {
  copyFileSync(join(repoRoot, `target/selara-codex.${suffix}`), join(notices, `selara-codex.${suffix}`));
}
