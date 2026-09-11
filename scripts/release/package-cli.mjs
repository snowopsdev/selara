// Build the complete native CLI distribution with the same stable identity as
// the desktop sidecars. Refuse publication conflicts, including on a retry.
import { execFileSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import { verifyCliArchive } from "./verify-cli-archive.mjs";
import { configuration } from "./build-desktop.mjs";
import { assetNames, checkSourceVersion, githubClient, readChecksum, sha256 } from "./desktop-release.mjs";
import { preflightUpdaterKey } from "./updater-artifacts.mjs";
import { validateAppleCredentials } from "./apple-signing.mjs";
import { createSigningKeychain, deleteSigningKeychain, registerSigningKeychain } from "./keychain-search-list.mjs";

const root = resolve(process.argv[3] || ".");
const tag = process.argv[2];
checkSourceVersion(root, tag);
const { cli } = assetNames(tag);
const client = githubClient(process.env.GITHUB_REPOSITORY);
const release = client.release(tag);
if (release.draft || release.prerelease) throw new Error("CLI publication requires an existing stable release");
const output = join(root, "target/cli-release");
mkdirSync(output, { recursive: true });
const present = new Set(release.assets.map(a => a.name));

if (present.has(cli)) {
  const archive = client.download(tag, cli, output);
  verifyCliArchive(archive, root, tag, process.env.APPLE_TEAM_ID);
  if (present.has(`${cli}.sha256`)) {
    const checksum = client.download(tag, `${cli}.sha256`, output);
    if (readChecksum(readFileSync(checksum, "utf8"), cli) !== sha256(archive)) throw new Error("Existing CLI checksum conflicts with its archive");
  } else {
    writeFileSync(`${archive}.sha256`, `${sha256(archive)}  ${cli}\n`);
    client.upload(tag, `${archive}.sha256`);
  }
  console.log("Verified and reused the published CLI archive");
} else {
  if (present.has(`${cli}.sha256`)) throw new Error("A checksum exists without its CLI archive; refusing to rebuild different contents");
  const settings = configuration(join(root, "apps/selara-desktop"), { ...process.env, SELARA_BUILD_MODE: "release" });
  const cliPath = join(root, "apps/selara-desktop/node_modules/@tauri-apps/cli/tauri.js");
  preflightUpdaterKey(cliPath, settings.pubkey, settings.environment);
  validateAppleCredentials(settings.environment);
  const run = (program, args, options = {}) => execFileSync(program, args, { cwd: root, encoding: "utf8", stdio: "pipe", ...options });
  const temp = mkdtempSync(join(tmpdir(), "selara-release-signing-"));
  chmodSync(temp, 0o700);
  const keychain = join(temp, "signing.keychain-db");
  const certificate = join(temp, "certificate.p12");
  const password = randomBytes(32).toString("hex");
  try {
    writeFileSync(certificate, Buffer.from(settings.environment.APPLE_CERTIFICATE, "base64"), { mode: 0o600 });
    // Suppress command exception details because security arguments contain passwords.
    try {
      createSigningKeychain(keychain, password);
      run("security", ["set-keychain-settings", "-lut", "21600", keychain]);
      run("security", ["unlock-keychain", "-p", password, keychain]);
      run("security", ["import", certificate, "-k", keychain, "-P", settings.environment.APPLE_CERTIFICATE_PASSWORD, "-T", "/usr/bin/codesign"]);
      run("security", ["set-key-partition-list", "-S", "apple-tool:,apple:,codesign:", "-s", "-k", password, keychain]);
      registerSigningKeychain(keychain);
      const identities = run("security", ["find-identity", "-v", "-p", "codesigning", keychain]);
      if (!identities.includes(`"${settings.environment.APPLE_SIGNING_IDENTITY}"`)) throw new Error("missing identity");
    } catch { throw new Error("Apple signing preflight failed; check the certificate, private key, and identity"); }
    run("sh", [join(root, "scripts/codex-runtime/build.sh")], { stdio: "inherit" });
    run("cargo", ["build", "--locked", "--release", "-p", "selara"], { stdio: "inherit" });
    const directory = join(output, basename(cli, ".tar.gz"));
    mkdirSync(directory, { recursive: true });
    for (const [from, name] of [["target/release/selara", "selara"], ["target/selara-codex", "selara-codex"], ["README.md", "README.md"], ["LICENSE", "LICENSE"], ...["LICENSE", "NOTICE", "provenance.json"].map(s => [`target/selara-codex.${s}`, `selara-codex.${s}`])]) copyFileSync(join(root, from), join(directory, name));
    for (const name of ["selara", "selara-codex"]) {
      run("codesign", ["--force", "--options", "runtime", "--timestamp", "--identifier", `dev.snowops.selara.${name}`, "--sign", settings.environment.APPLE_SIGNING_IDENTITY, "--keychain", keychain, join(directory, name)]);
      run("codesign", ["--verify", "--strict", join(directory, name)]);
    }
    const archive = join(output, cli);
    if (existsSync(archive)) rmSync(archive);
    run("tar", ["--disable-copyfile", "-czf", archive, "-C", output, basename(directory)]);
    verifyCliArchive(archive, root, tag, process.env.APPLE_TEAM_ID);
    writeFileSync(`${archive}.sha256`, `${sha256(archive)}  ${cli}\n`);
    client.upload(tag, archive);
    client.upload(tag, `${archive}.sha256`);
    const published = client.download(tag, cli, join(output, "verified"));
    if (sha256(published) !== sha256(archive)) throw new Error("Published CLI archive changed");
  } finally {
    try { deleteSigningKeychain(keychain); } catch { /* May not have been created. */ }
    rmSync(temp, { recursive: true, force: true });
  }
}
