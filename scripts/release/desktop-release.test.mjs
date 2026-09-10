import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import test from "node:test";
import { signingFixture } from "./signing-fixture.mjs";
import { assetNames, checkSourceVersion, checkTapVersion, prepareRelease, publishRelease, readChecksum, resolveRelease, sha256, stageRelease, verifyRelease, versionFromTag } from "./desktop-release.mjs";

const tag = "v0.4.0";
const names = assetNames(tag);
const commit = "a".repeat(40);

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), "selara-release-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const source = join(root, "source");
  const output = join(root, "artifacts");
  const file = (path, content) => {
    mkdirSync(resolve(path, ".."), { recursive: true });
    writeFileSync(path, content);
  };
  file(join(source, "Cargo.toml"), '[workspace.package]\nversion = "0.4.0"\n[dependencies]\n');
  file(join(source, "apps/selara-desktop/package.json"), '{"version":"0.4.0"}');
  file(join(source, "apps/selara-desktop/src-tauri/tauri.conf.json"), '{"version":"0.4.0"}');
  file(join(output, names.cli), "published CLI bytes");
  const archiveHash = sha256(join(output, names.cli));
  const remote = new Map([
    [names.cli, Buffer.from("published CLI bytes")],
    [`${names.cli}.sha256`, Buffer.from(`${archiveHash}  dist/${names.cli}\n`)],
  ]);
  const uploads = [];
  const client = {
    immutable: false,
    draft: false,
    repository: "owner/repo",
    release: () => ({ tag_name: tag, published_at: "2026-09-10T12:00:00Z", body: "Release notes", draft: client.draft, prerelease: false, immutable: client.immutable, assets: [...remote.keys()].map((name) => ({ name, state: "uploaded" })) }),
    download(_tag, name, directory) {
      assert.equal(_tag, tag);
      if (!remote.has(name)) throw new Error(`Missing ${name}`);
      const path = join(directory, name);
      file(path, remote.get(name));
      return path;
    },
    upload(_tag, path) {
      assert.equal(_tag, tag);
      const name = basename(path);
      assert.ok(!remote.has(name), "must never overwrite a remote asset");
      remote.set(name, readFileSync(path));
      uploads.push(name);
    },
    api(path) {
      if (path === `git/ref/tags/${tag}`) return { object: { type: "commit", sha: commit } };
      if (path === `compare/${commit}...main`) return { status: "ahead" };
      throw new Error(`Unexpected API call ${path}`);
    },
  };
  const stage = (notarized = false, options = {}) => {
    for (const path of ["scripts/release/render-homebrew.sh", "homebrew/Casks/selara.rb.tmpl", "homebrew/Formula/selara.rb.tmpl"]) {
      file(join(source, path), readFileSync(new URL(`../../${path}`, import.meta.url)));
    }
    stageRelease(tag, source, output, (binary, args) => {
      if (binary === "hdiutil") { assert.deepEqual(args, ["verify", join(output, names.dmg)]); return; }
      execFileSync(binary, args, { stdio: "pipe" });
    }, () => notarized, { prepareArchive: () => {}, ...options });
  };
  const builtDmg = () => file(join(source, "target/release/bundle/dmg/Selara.dmg"), "newly built DMG bytes");
  return { root, source, output, client, remote, uploads, file, stage, builtDmg };
}

test("rejects non-release refs before any GitHub call", () => {
  for (const value of ["main", "--help", "v0.4.0/extra", "v0.4.0\n", "v00.4.0", "v0.4.0-rc.1", "$(cmd)", undefined]) {
    assert.throws(() => resolveRelease(value, { release() { assert.fail("unexpected network call"); } }), /vX.Y.Z/);
  }
  assert.equal(versionFromTag(tag), "0.4.0");
});

test("resolves lightweight and annotated release tags and rejects unrelated history", (t) => {
  const f = fixture(t);
  assert.equal(resolveRelease(tag, f.client), commit);
  const api = f.client.api;
  f.client.api = (path) => path === `git/ref/tags/${tag}` ? { object: { type: "tag", sha: "b".repeat(40) } }
    : path === `git/tags/${"b".repeat(40)}` ? { object: { type: "commit", sha: commit } } : api(path);
  assert.equal(resolveRelease(tag, f.client), commit);
  f.client.api = (path) => path.startsWith("compare/") ? { status: "diverged" } : api(path);
  assert.throws(() => resolveRelease(tag, f.client), /main's history/);
});

test("requires a published release and matching workspace, npm, and Tauri versions", (t) => {
  const f = fixture(t);
  f.client.draft = true;
  assert.throws(() => resolveRelease(tag, f.client), /published stable/);
  f.client.draft = false;
  checkSourceVersion(f.source, tag);
  f.file(join(f.source, "apps/selara-desktop/src-tauri/tauri.conf.json"), '{"version":"0.3.0"}');
  assert.throws(() => prepareRelease(tag, f.source, f.output, f.client), /versions must all match/);
});

test("requires both CLI assets and verifies the published archive before building", (t) => {
  const f = fixture(t);
  assert.equal(prepareRelease(tag, f.source, f.output, f.client).buildRequired, true);
  f.remote.set(names.cli, Buffer.from("corrupt archive"));
  assert.throws(() => prepareRelease(tag, f.source, f.output, f.client), /checksum does not match/);
  f.remote.delete(`${names.cli}.sha256`);
  assert.throws(() => prepareRelease(tag, f.source, f.output, f.client), /Missing prerequisite/);
});

test("refuses recovery when GitHub has locked an incomplete release", (t) => {
  const f = fixture(t);
  f.client.immutable = true;
  assert.throws(() => prepareRelease(tag, f.source, f.output, f.client), /immutable/);
});

test("publishes a complete release and verifies all six downloaded assets", (t) => {
  const f = fixture(t);
  prepareRelease(tag, f.source, f.output, f.client);
  f.builtDmg();
  f.stage();
  assert.match(readFileSync(join(f.output, "selara-cask.rb"), "utf8"), /not notarized by Apple/);
  assert.deepEqual(publishRelease(tag, f.output, f.client), names.desktop);
  verifyRelease(tag, f.output, f.client);
  assert.equal(f.remote.size, 6);
  assert.equal(f.remote.get(names.cli).toString(), "published CLI bytes");
  f.remote.set(`${names.dmg}.sha256`, Buffer.from(`${"f".repeat(64)}  ${names.dmg}\n`));
  assert.throws(() => verifyRelease(tag, f.output, f.client), /checksum mismatch/);
});

test("partial and repeated recovery reuse the original DMG without overwriting it", (t) => {
  const f = fixture(t);
  f.remote.set(names.dmg, Buffer.from("original published DMG bytes"));
  assert.equal(prepareRelease(tag, f.source, f.output, f.client).buildRequired, false);
  f.builtDmg();
  f.stage();
  assert.equal(readFileSync(join(f.output, names.dmg), "utf8"), "original published DMG bytes");
  assert.deepEqual(publishRelease(tag, f.output, f.client), names.desktop.slice(1));
  assert.deepEqual(publishRelease(tag, f.output, f.client), []);
  verifyRelease(tag, f.output, f.client);
  assert.equal(f.uploads.length, 3);
});

test("an interrupted upload can resume without changing the DMG or CLI", (t) => {
  const f = fixture(t);
  prepareRelease(tag, f.source, f.output, f.client);
  f.builtDmg();
  f.stage();
  const upload = f.client.upload;
  f.client.upload = (_tag, path) => {
    if (f.uploads.length === 2) throw new Error("network disconnected");
    upload(_tag, path);
  };
  assert.throws(() => publishRelease(tag, f.output, f.client), /disconnected/);
  f.client.upload = upload;
  assert.equal(prepareRelease(tag, f.source, f.output, f.client).buildRequired, false);
  f.stage();
  assert.deepEqual(publishRelease(tag, f.output, f.client), names.desktop.slice(2));
  verifyRelease(tag, f.output, f.client);
});

test("checks all conflicts before publishing any missing assets", (t) => {
  const f = fixture(t);
  prepareRelease(tag, f.source, f.output, f.client);
  f.builtDmg();
  f.stage();
  f.remote.set("selara-formula.rb", Buffer.from("conflicting formula"));
  assert.throws(() => publishRelease(tag, f.output, f.client), /Conflicting published asset/);
  assert.deepEqual(f.uploads, []);
});

test("only verified notarization removes the cask caveat", (t) => {
  const f = fixture(t);
  prepareRelease(tag, f.source, f.output, f.client);
  f.builtDmg();
  f.stage(true);
  assert.doesNotMatch(readFileSync(join(f.output, "selara-cask.rb"), "utf8"), /not notarized by Apple/);
});

test("checksum validation permits the legacy dist path but rejects another filename", () => {
  assert.equal(readChecksum(`${"a".repeat(64)}  dist/${names.dmg}\n`, names.dmg), "a".repeat(64));
  assert.throws(() => readChecksum(`${"a".repeat(64)}  wrong.dmg`, names.dmg), /Invalid checksum/);
});

test("older recovery cannot downgrade a newer tap even after a stale latest check", (t) => {
  const f = fixture(t);
  const tap = join(f.root, "tap");
  assert.equal(checkTapVersion(tag, tap, "v0.5.0"), false);
  f.file(join(tap, "Casks/selara.rb"), 'version "0.5.0"\n');
  assert.throws(() => checkTapVersion(tag, tap, tag), /Refusing to downgrade/);
  f.file(join(tap, "Casks/selara.rb"), 'version "0.3.9"\n');
  assert.equal(checkTapVersion(tag, tap, tag), true);
  f.file(join(tap, "Formula/selara.rb"), 'version "0.4.1"\n');
  assert.throws(() => checkTapVersion(tag, tap, tag), /Refusing to downgrade/);
});

function updaterFixture(t) {
  const f = fixture(t);
  const names = assetNames(tag, true);
  const bytes = Buffer.from("exact published app archive");
  const keys = signingFixture(bytes);
  f.file(join(f.source, "apps/selara-desktop/src-tauri/tauri.conf.json"), JSON.stringify({version:"0.4.0",bundle:{createUpdaterArtifacts:true},plugins:{updater:{pubkey:keys.publicKey}}}));
  f.remote.set(names.dmg, Buffer.from("verified published DMG"));
  f.remote.set(names.archive, bytes);
  f.remote.set(`${names.archive}.sig`, Buffer.from(keys.signature));
  return {...f, names, bytes, keys};
}

test("updater recovery reuses signed payload, verifies nine assets, and publishes manifest last", t => {
  const f = updaterFixture(t);
  assert.equal(prepareRelease(tag, f.source, f.output, f.client).buildRequired, false);
  f.stage(true);
  publishRelease(tag, f.output, f.client);
  assert.equal(f.uploads.at(-1), "latest.json");
  assert.equal(f.remote.size, 9);
  assert.equal(f.remote.get(f.names.archive).toString(), f.bytes.toString());
  verifyRelease(tag, f.output, f.client);
  assert.deepEqual(publishRelease(tag, f.output, f.client), []);
  const manifest = JSON.parse(f.remote.get("latest.json"));
  assert.equal(manifest.platforms["darwin-aarch64"].signature, f.keys.signature);
  assert.match(manifest.platforms["darwin-aarch64"].url, /releases\/download\/v0\.4\.0/);
});

test("updater recovery rejects missing notarization, tampered payload, and conflicting feed", t => {
  const f = updaterFixture(t);
  prepareRelease(tag, f.source, f.output, f.client);
  assert.throws(() => f.stage(), /verified notarized/);
  f.stage(true);
  f.remote.set("latest.json", Buffer.from("conflicting manifest"));
  assert.throws(() => publishRelease(tag, f.output, f.client), /Conflicting published asset/);
  assert.deepEqual(f.uploads, []);
  f.remote.delete("latest.json");
  f.file(join(f.output, f.names.archive), "tampered");
  assert.throws(() => f.stage(true), /signature verification/);
});

test("an interrupted updater publication can resume before making its feed visible", t => {
  const f = updaterFixture(t);
  prepareRelease(tag, f.source, f.output, f.client); f.stage(true);
  const download = f.client.download;
  f.client.download = (tag, name, directory) => {
    if (directory.endsWith("before-manifest")) throw new Error("network interrupted");
    return download(tag, name, directory);
  };
  assert.throws(() => publishRelease(tag, f.output, f.client), /interrupted/);
  assert.equal(f.remote.has("latest.json"), false);
  f.client.download = download;
  prepareRelease(tag, f.source, f.output, f.client); f.stage(true);
  assert.deepEqual(publishRelease(tag, f.output, f.client), ["latest.json"]);
  verifyRelease(tag, f.output, f.client);
});

test("an existing payload without a signature is checked against the DMG before signing", t => {
  const f = updaterFixture(t); f.remote.delete(`${f.names.archive}.sig`);
  prepareRelease(tag, f.source, f.output, f.client);
  let signed = false;
  assert.throws(() => f.stage(true, {
    prepareArchive: () => { throw new Error("Updater archive does not match the verified published DMG"); },
    sign: () => { signed = true; },
  }), /does not match/);
  assert.equal(signed, false); assert.deepEqual(f.uploads, []);
});
