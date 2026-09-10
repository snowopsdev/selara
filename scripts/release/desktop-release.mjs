// Recovery uses this tooling from the workflow commit and app source from the
// release tag. Never replace an existing release asset with a fresh build.
import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { appendFileSync, copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { basename, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export function versionFromTag(tag) {
  if (!/^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(tag)) {
    throw new Error("Expected an existing release tag in vX.Y.Z format");
  }
  return tag.slice(1);
}

export function assetNames(tag) {
  const version = versionFromTag(tag);
  const cli = `selara-${version}-macos-arm64.tar.gz`;
  const dmg = `Selara-${version}-macos-arm64.dmg`;
  return { cli, dmg, desktop: [dmg, `${dmg}.sha256`, "selara-cask.rb", "selara-formula.rb"] };
}

export function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

export function readChecksum(text, name) {
  const match = text.trim().match(/^([a-fA-F0-9]{64})\s+\*?([^\r\n]+)$/);
  if (!match || basename(match[2]) !== name) throw new Error(`Invalid checksum file for ${name}`);
  return match[1].toLowerCase();
}

function checkRelease(release, tag) {
  if (release.tag_name !== tag || release.draft || release.prerelease) {
    throw new Error(`Expected a published stable release for ${tag}`);
  }
}

export function checkSourceVersion(source, tag) {
  const version = versionFromTag(tag);
  const desktop = join(source, "apps/selara-desktop");
  const pkg = JSON.parse(readFileSync(join(desktop, "package.json"), "utf8"));
  const config = JSON.parse(readFileSync(join(desktop, "src-tauri/tauri.conf.json"), "utf8"));
  const cargo = readFileSync(join(source, "Cargo.toml"), "utf8");
  const workspace = cargo.match(/\[workspace\.package\]([^]*?)(?=\n\[|$)/)?.[1];
  const cargoVersion = workspace?.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  if ([pkg.version, config.version, cargoVersion].some((value) => value !== version)) {
    throw new Error(`Application versions must all match ${tag}`);
  }
}

function command(binary, args, options = {}) {
  return execFileSync(binary, args, { encoding: "utf8", stdio: ["ignore", "pipe", "inherit"], ...options });
}

export function githubClient(repo) {
  if (!/^[\w.-]+\/[\w.-]+$/.test(repo ?? "")) throw new Error("GITHUB_REPOSITORY is required");
  const api = (path) => JSON.parse(command("gh", ["api", `repos/${repo}/${path}`]));
  return {
    api,
    release: (tag) => api(`releases/tags/${tag}`),
    download(tag, name, directory) {
      mkdirSync(directory, { recursive: true });
      command("gh", ["release", "download", tag, "--repo", repo, "--pattern", name, "--dir", directory, "--clobber"]);
      return join(directory, name);
    },
    upload(tag, path) {
      command("gh", ["release", "upload", tag, path, "--repo", repo]);
    },
  };
}

export function resolveRelease(tag, client) {
  versionFromTag(tag);
  checkRelease(client.release(tag), tag);
  let object = client.api(`git/ref/tags/${tag}`).object;
  for (let depth = 0; object.type === "tag" && depth < 5; depth++) {
    object = client.api(`git/tags/${object.sha}`).object;
  }
  if (object.type !== "commit" || !/^[a-f0-9]{40}$/.test(object.sha)) throw new Error("Tag must resolve to a commit");
  const comparison = client.api(`compare/${object.sha}...main`);
  if (!["ahead", "identical"].includes(comparison.status)) throw new Error("Release tag must belong to main's history");
  return object.sha;
}

export function prepareRelease(tag, source, output, client) {
  checkSourceVersion(source, tag);
  const names = assetNames(tag);
  const release = client.release(tag);
  checkRelease(release, tag);
  const present = new Set(release.assets.filter((asset) => asset.state === "uploaded").map((asset) => asset.name));
  for (const name of [names.cli, `${names.cli}.sha256`]) {
    if (!present.has(name)) throw new Error(`Missing prerequisite release asset: ${name}`);
    client.download(tag, name, output);
  }
  const expected = readChecksum(readFileSync(join(output, `${names.cli}.sha256`), "utf8"), names.cli);
  if (sha256(join(output, names.cli)) !== expected) throw new Error("Published CLI archive checksum does not match");
  if (release.immutable && names.desktop.some((name) => !present.has(name))) {
    throw new Error("Cannot recover missing assets on an immutable release");
  }
  if (present.has(names.dmg)) client.download(tag, names.dmg, output);
  return { buildRequired: !present.has(names.dmg) };
}

export function stageRelease(tag, source, output, run = command, notarized = (path) => {
  return spawnSync("xcrun", ["stapler", "validate", path], { stdio: "ignore" }).status === 0;
}) {
  const names = assetNames(tag);
  const dmg = join(output, names.dmg);
  if (!existsSync(dmg)) {
    const directory = join(source, "target/release/bundle/dmg");
    const files = readdirSync(directory).filter((name) => name.endsWith(".dmg"));
    if (files.length !== 1) throw new Error("Expected exactly one built DMG");
    copyFileSync(join(directory, files[0]), dmg);
  }
  run("hdiutil", ["verify", dmg]);
  const dmgSha = sha256(dmg);
  const cliSha = sha256(join(output, names.cli));
  writeFileSync(`${dmg}.sha256`, `${dmgSha}  ${names.dmg}\n`);
  // Query the actual DMG, rather than treating a signing identity as proof of
  // notarization. This also preserves the caveat when reusing an old DMG.
  run("bash", [join(source, "scripts/release/render-homebrew.sh"), versionFromTag(tag), dmgSha, cliSha, join(output, "homebrew"), notarized(dmg) ? "signed" : "unsigned"]);
  copyFileSync(join(output, "homebrew/Casks/selara.rb"), join(output, "selara-cask.rb"));
  copyFileSync(join(output, "homebrew/Formula/selara.rb"), join(output, "selara-formula.rb"));
}

export function publishRelease(tag, output, client) {
  const names = assetNames(tag);
  const release = client.release(tag);
  checkRelease(release, tag);
  const present = new Set(release.assets.map((asset) => asset.name));
  const missing = [];
  // Check every conflict before uploading anything. Reruns use the original
  // DMG bytes and must agree with all already-published companion files.
  for (const name of names.desktop) {
    const local = join(output, name);
    if (!existsSync(local)) throw new Error(`Missing staged asset: ${name}`);
    if (!present.has(name)) {
      missing.push(local);
      continue;
    }
    const remote = client.download(tag, name, join(output, "existing"));
    const same = name === `${names.dmg}.sha256`
      ? readChecksum(readFileSync(remote, "utf8"), names.dmg) === sha256(join(output, names.dmg))
      : sha256(remote) === sha256(local);
    if (!same) throw new Error(`Conflicting published asset: ${name}; nothing was overwritten`);
  }
  if (release.immutable && missing.length) throw new Error("Cannot upload to an immutable release");
  for (const path of missing) client.upload(tag, path);
  return missing.map((path) => basename(path));
}

export function verifyRelease(tag, output, client) {
  const names = assetNames(tag);
  const release = client.release(tag);
  checkRelease(release, tag);
  for (const name of [names.cli, `${names.cli}.sha256`, ...names.desktop]) {
    if (!release.assets.some((asset) => asset.name === name && asset.state === "uploaded")) {
      throw new Error(`Missing published asset: ${name}`);
    }
    const downloaded = client.download(tag, name, join(output, "verified"));
    if (name.endsWith(".sha256")) {
      const artifact = name.slice(0, -7);
      if (readChecksum(readFileSync(downloaded, "utf8"), artifact) !== sha256(join(output, "verified", artifact))) {
        throw new Error(`Published checksum mismatch: ${artifact}`);
      }
    } else if (sha256(downloaded) !== sha256(join(output, name))) {
      throw new Error(`Published asset changed: ${name}`);
    }
  }
}

export function checkTapVersion(tag, tapDirectory, latestTag) {
  const version = versionFromTag(tag);
  if (tag !== latestTag) return false;
  const parts = version.split(".").map(BigInt);
  for (const file of ["Casks/selara.rb", "Formula/selara.rb"]) {
    const path = join(tapDirectory, file);
    if (!existsSync(path)) continue;
    const current = readFileSync(path, "utf8").match(/^\s*version "([^"]+)"/m)?.[1];
    const currentParts = versionFromTag(`v${current}`).split(".").map(BigInt);
    for (let i = 0; i < parts.length; i++) {
      if (parts[i] < currentParts[i]) throw new Error(`Refusing to downgrade Homebrew from ${current} to ${version}`);
      if (parts[i] > currentParts[i]) break;
    }
  }
  return true;
}

function output(name, value) {
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, `${name}=${value}\n`);
  console.log(`${name}=${value}`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const [phase, tag, sourceArg = "source", outputArg = "artifacts"] = process.argv.slice(2);
    versionFromTag(tag);
    const source = resolve(sourceArg);
    const directory = resolve(outputArg);
    const client = githubClient(process.env.GITHUB_REPOSITORY);
    if (phase === "resolve") output("sha", resolveRelease(tag, client));
    else if (phase === "prepare") output("build_required", prepareRelease(tag, source, directory, client).buildRequired);
    else if (phase === "stage") stageRelease(tag, source, directory);
    else if (phase === "publish") console.log(`Uploaded: ${publishRelease(tag, directory, client).join(", ") || "none; assets already match"}`);
    else if (phase === "verify") { verifyRelease(tag, directory, client); console.log(`Verified all release assets for ${tag}`); }
    else if (phase === "check-tap") output("publish", checkTapVersion(tag, source, client.api("releases/latest").tag_name));
    else throw new Error("Expected resolve, prepare, stage, publish, verify, or check-tap");
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
