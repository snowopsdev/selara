import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";
import { fileURLToPath } from "node:url";
import { developerIdRequirement } from "./apple-signature.mjs";
/** Executes a verification command with bounded, captured output. */
const command = (program, args) => execFileSync(program, args, { encoding: "utf8", stdio: "pipe", timeout: 60_000 });

/**
 * Verifies the contents, signatures, versions, and provenance of a recovered CLI archive.
 *
 * @param {string} archive - Path to the archive under verification.
 * @param {string} root - Path to the matching release source tree.
 * @param {string} tag - Release tag whose version the archive must contain.
 * @param {string} teamId - Expected Apple Developer Team identifier.
 * @param {(program: string, args: string[]) => string} run - Command runner used for verification.
 */
export function verifyCliArchive(archive, root, tag, teamId, run = command) {
  const requirement = developerIdRequirement(teamId);
  const cli = `selara-${tag.slice(1)}-macos-arm64.tar.gz`;
  const directory = mkdtempSync(join(tmpdir(), "selara-cli-verify-"));
  try {
    run("python3", [fileURLToPath(new URL("./extract-archive.py", import.meta.url)), archive, directory, basename(cli, ".tar.gz")]);
    const content = join(directory, basename(cli, ".tar.gz"));
    const required = ["selara", "selara-codex", "selara-codex.LICENSE", "selara-codex.NOTICE", "selara-codex.provenance.json"];
    // Recovery uses the release's original source. Older releases did not
    // bundle the orb, so require its license only when that source includes it.
    if (existsSync(join(root, "apps/selara/native/ThinkingOrbsKit"))) required.push("thinking-orbs.LICENSE");
    for (const name of required) if (!existsSync(join(content, name))) throw new Error(`Missing CLI archive file: ${name}`);
    for (const binary of ["selara", "selara-codex"]) {
      run("codesign", ["--verify", "--strict", join(content, binary)]);
      // Verify a requirement instead of parsing localized diagnostic output.
      run("codesign", ["--verify", "--strict", "-R", requirement, join(content, binary)]);
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
