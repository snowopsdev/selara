import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";
import { fileURLToPath } from "node:url";
const command = (program, args) => execFileSync(program, args, { encoding: "utf8", stdio: "pipe", timeout: 60_000 });

export function verifyCliArchive(archive, root, tag, teamId, run = command) {
  const cli = `selara-${tag.slice(1)}-macos-arm64.tar.gz`;
  const directory = mkdtempSync(join(tmpdir(), "selara-cli-verify-"));
  try {
    run("python3", [fileURLToPath(new URL("./extract-archive.py", import.meta.url)), archive, directory, basename(cli, ".tar.gz")]);
    const content = join(directory, basename(cli, ".tar.gz"));
    for (const name of ["selara", "selara-codex", "selara-codex.LICENSE", "selara-codex.NOTICE", "selara-codex.provenance.json"]) if (!existsSync(join(content, name))) throw new Error(`Missing CLI archive file: ${name}`);
    for (const binary of ["selara", "selara-codex"]) {
      run("codesign", ["--verify", "--strict", join(content, binary)]);
      // Verify a requirement instead of parsing localized diagnostic output.
      if (!/^[A-Z0-9]{10}$/.test(teamId || "")) throw new Error("APPLE_TEAM_ID is required to verify a recovered CLI archive");
      const details = `anchor apple generic and certificate leaf[subject.OU] = "${teamId}" and certificate leaf[field.1.2.840.113635.100.6.1.13] exists`;
      run("codesign", ["--verify", "--strict", "-R", details, join(content, binary)]);
    }
    if (run(join(content, "selara"), ["--version"]).trim() !== `selara ${tag.slice(1)}`) throw new Error("Recovered CLI version does not match release");
    if (run(join(content, "selara-codex"), ["--version"]).trim() !== "selara-codex 0.153.4") throw new Error("Unexpected bundled Codex version");
    const provenance = JSON.parse(readFileSync(join(content, "selara-codex.provenance.json")));
    const pinned = readFileSync(join(root, "vendor/codex-runtime/runtime.toml"), "utf8");
    for (const key of ["source_revision", "patches_sha256", "target", "minimum_macos"]) {
      if (provenance[key] !== pinned.match(new RegExp(`^${key} = "([^"]+)"`, "m"))?.[1]) throw new Error(`Recovered runtime provenance mismatch: ${key}`);
    }
  } finally { rmSync(directory, { recursive: true, force: true }); }
}
