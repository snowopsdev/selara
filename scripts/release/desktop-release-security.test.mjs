import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { archivePublishedApp, assetNames, stageRelease } from "./desktop-release.mjs";
import { verifyUpdaterSignature } from "./updater-artifacts.mjs";
import { signingFixture } from "./signing-fixture.mjs";

const expectedTeam = "SELARATEAM";
const foreignTeam = "FOREIGN001";
const tag = "v0.4.1";
const version = tag.slice(1);
const codePaths = ["", "Contents/MacOS/selara-desktop", "Contents/MacOS/selara", "Contents/MacOS/selara-codex"];

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), "selara-release-security-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const source = fileURLToPath(new URL("../../", import.meta.url));
  const app = join(root, "published/Selara.app");
  mkdirSync(join(app, "Contents/MacOS"), { recursive: true });
  const signaturePath = (path) => statSync(path).isDirectory() ? join(path, "Contents/fixture-signature.json") : path;
  const setIdentity = (codePath, identity) => writeFileSync(signaturePath(join(app, codePath)), JSON.stringify({ team: expectedTeam, apple: true, developerId: true, ...identity }));
  writeFileSync(join(app, "Contents/Info.plist"), "Fixture version and bundle identifier");
  for (const path of codePaths.slice(1)) writeFileSync(join(app, path), "", { mode: 0o755 });
  for (const path of codePaths) setIdentity(path, {});
  const output = join(root, "artifacts");
  mkdirSync(output);
  const names = assetNames(tag, true);
  const dmg = join(output, names.dmg);
  const archive = join(output, names.archive);
  writeFileSync(dmg, "Published fixture DMG");
  writeFileSync(join(output, names.cli), "Published fixture CLI");
  const keys = signingFixture(Buffer.from("fixture"));
  writeFileSync(join(output, "release-plan.json"), JSON.stringify({ updater: true, pubkey: keys.publicKey, repository: "owner/repo", publishedAt: "2026-09-10T12:00:00Z" }));
  const calls = [];
  let signerCalls = 0;
  const sign = (_cli, path) => {
    signerCalls++;
    writeFileSync(`${path}.sig`, keys.signBytes(readFileSync(path)));
  };
  // Only platform signing/notarization and DMG mounting are simulated. Copies,
  // tar extraction, byte comparison, updater signatures, and staging are real.
  const run = (program, args) => {
    calls.push({ program, args });
    if (program === "hdiutil") {
      if (args[0] === "attach") cpSync(app, join(args[args.indexOf("-mountpoint") + 1], "Selara.app"), { recursive: true });
      return "";
    }
    if (program === "codesign") {
      const identity = JSON.parse(readFileSync(signaturePath(args.at(-1)), "utf8"));
      const index = args.indexOf("-R");
      if (index !== -1) {
        const requirement = args[index + 1];
        assert.ok(requirement.startsWith("="), "codesign requires '=' for inline requirements");
        const team = requirement.match(/certificate leaf\[subject\.OU\] = "([A-Z0-9]{10})"/)?.[1];
        if (team !== identity.team) throw new Error("Signing team does not satisfy requirement");
        if (requirement.includes("anchor apple generic") && !identity.apple) throw new Error("Signature is not Apple-anchored");
        if (requirement.includes("certificate leaf[field.1.2.840.113635.100.6.1.13] exists") && !identity.developerId) throw new Error("Signature is not Developer ID Application");
      }
      return "";
    }
    if (program === "xcrun") return ""; // Valid staples alone must not establish team trust.
    if (program === "/usr/libexec/PlistBuddy") return args[1].includes("Version") ? version : "dev.snowops.selara";
    if (program === "ditto") { cpSync(args[0], args[1], { recursive: true }); return ""; }
    // GNU tar has no --disable-copyfile; the environment disables macOS sidecars.
    return execFileSync(program, program === "tar" ? args.filter(value => value !== "--disable-copyfile") : args, { encoding: "utf8", stdio: "pipe", env: { ...process.env, COPYFILE_DISABLE: "1" } });
  };
  const stage = (options = {}) => stageRelease(tag, source, output, run, () => true, { teamId: expectedTeam, sign, ...options });
  const pack = () => execFileSync("tar", ["-czf", archive, "-C", join(root, "published"), "Selara.app"], { env: { ...process.env, COPYFILE_DISABLE: "1" } });
  const assertNotSigned = () => {
    assert.equal(signerCalls, 0);
    assert.equal(existsSync(`${archive}.sig`), false);
    assert.equal(existsSync(join(output, "latest.json")), false);
  };
  return { app, archive, calls, dmg, keys, output, pack, run, setIdentity, sign, stage, assertNotSigned, signerCalls: () => signerCalls };
}

for (const existingArchive of [false, true]) {
  test(`a valid notarized app from another team cannot reach updater signing (archive present: ${existingArchive})`, t => {
    const f = fixture(t);
    for (const path of codePaths) f.setIdentity(path, { team: foreignTeam });
    if (existingArchive) f.pack();
    assert.throws(() => f.stage(), /Signing team/);
    f.assertNotSigned();
    assert.ok(f.calls.some(({ program, args }) => program === "hdiutil" && args[0] === "detach"));
    assert.ok(!f.calls.some(({ program }) => program === "ditto"));
  });
}

for (const path of codePaths.slice(1)) {
  test(`the app's trusted team cannot hide a foreign signature on ${path}`, t => {
    const f = fixture(t);
    f.setIdentity(path, { team: foreignTeam });
    assert.throws(() => f.stage(), /Signing team/);
    f.assertNotSigned();
  });
}

test("matching team text without an Apple anchor or Developer ID certificate is rejected", t => {
  for (const identity of [{ apple: false }, { developerId: false }]) {
    const f = fixture(t);
    f.setIdentity("", identity);
    assert.throws(() => f.stage(), /not Apple-anchored|not Developer ID/);
    f.assertNotSigned();
  }
});

test("missing bundled code cannot be promoted into an updater", t => {
  const f = fixture(t);
  rmSync(join(f.app, "Contents/MacOS/selara-codex"));
  assert.throws(() => f.stage(), /ENOENT/);
  f.assertNotSigned();
});

for (const path of codePaths) {
  test(`an existing archive is independently checked for foreign code at ${path || "Selara.app"}`, t => {
    const f = fixture(t);
    f.setIdentity(path, { team: foreignTeam });
    f.pack();
    f.setIdentity(path, {}); // The mounted DMG now satisfies the expected team.
    assert.throws(() => f.stage(), /Signing team/);
    f.assertNotSigned();
    assert.ok(f.calls.some(({ program, args }) => program === "codesign" && args.at(-1).includes("/extracted/")));
  });
}

test("updater staging and direct archive recovery reject absent or malformed trusted team configuration before using artifacts", t => {
  const f = fixture(t);
  for (const teamId of ["", null, 1234567890, [], "TOOSHORT", "FOREIGN001\n", "foreign001", '" or true']) {
    assert.throws(() => f.stage({ teamId }), /APPLE_TEAM_ID/);
    assert.throws(() => archivePublishedApp(f.dmg, f.archive, version, f.run, teamId), /APPLE_TEAM_ID/);
  }
  assert.deepEqual(f.calls, []);
  f.assertNotSigned();
});

for (const existingArchive of [false, true]) {
  test(`expected-team recovery signs exact app bytes and reuses them on repeat (archive present: ${existingArchive})`, t => {
    const f = fixture(t);
    if (existingArchive) f.pack();
    const published = existingArchive ? readFileSync(f.archive) : null;
    f.stage();
    const archive = readFileSync(f.archive);
    if (published) assert.deepEqual(archive, published);
    const signature = readFileSync(`${f.archive}.sig`, "utf8");
    verifyUpdaterSignature(archive, signature, f.keys.publicKey);
    assert.equal(f.signerCalls(), 1);
    const manifest = readFileSync(join(f.output, "latest.json"));
    assert.equal(f.calls.filter(({ program }) => program === "codesign").length, codePaths.length * 2);
    f.stage();
    assert.equal(f.signerCalls(), 1);
    assert.deepEqual(readFileSync(f.archive), archive);
    assert.deepEqual(readFileSync(join(f.output, "latest.json")), manifest);
  });
}

test("a pre-existing updater signature does not skip the Apple team requirement", t => {
  const f = fixture(t);
  f.setIdentity("", { team: foreignTeam });
  f.pack();
  f.sign(null, f.archive);
  assert.throws(() => f.stage(), /Signing team/);
  assert.equal(f.signerCalls(), 1, "the rejected recovery must not sign again");
  assert.equal(existsSync(join(f.output, "latest.json")), false);
});
