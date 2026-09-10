import assert from "node:assert/strict";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { buildArguments, configuration, runBuild } from "./build-desktop.mjs";

async function desktopFixture({ artifacts = false, pubkey = "REPLACE_WITH_TAURI_UPDATER_PUBKEY", cliSource = "" } = {}) {
  const directory = await mkdtemp(join(tmpdir(), "selara-desktop-"));
  const cliDirectory = join(directory, "node_modules", "@tauri-apps", "cli");
  await mkdir(join(directory, "src-tauri"), { recursive: true });
  await mkdir(cliDirectory, { recursive: true });
  await writeFile(join(cliDirectory, "tauri.js"), cliSource);
  await writeFile(join(directory, "src-tauri", "tauri.conf.json"), JSON.stringify({
    bundle: { createUpdaterArtifacts: artifacts },
    plugins: { updater: { pubkey } },
  }));
  return directory;
}

test("omits absent and empty signing/notarization variables and uses ad hoc overlay", async (t) => {
  const directory = await desktopFixture();
  t.after(() => rm(directory, { recursive: true, force: true }));
  const result = configuration(directory, {
    PATH: "/usr/bin",
    APPLE_CERTIFICATE: "",
    APPLE_CERTIFICATE_PASSWORD: "",
    APPLE_SIGNING_IDENTITY: "",
    APPLE_ID: "",
    APPLE_PASSWORD: "",
    APPLE_TEAM_ID: "",
  });
  assert.equal(result.environment.APPLE_CERTIFICATE, undefined);
  assert.equal(result.environment.APPLE_PASSWORD, undefined);
  assert.equal(result.overlay.bundle.macOS.signingIdentity, "-");
});

test("preserves the exact certificate password for complete Developer ID signing", async (t) => {
  const directory = await desktopFixture();
  t.after(() => rm(directory, { recursive: true, force: true }));
  const result = configuration(directory, {
    APPLE_CERTIFICATE: "base64-cert",
    APPLE_CERTIFICATE_PASSWORD: "  exact password  ",
    APPLE_SIGNING_IDENTITY: "Developer ID Application: Selara",
    APPLE_ID: "user@example.test",
    APPLE_PASSWORD: "app-password",
    APPLE_TEAM_ID: "TEAM123",
  });
  assert.equal(result.environment.APPLE_CERTIFICATE_PASSWORD, "  exact password  ");
  assert.equal(result.overlay.bundle.macOS.signingIdentity, "Developer ID Application: Selara");
});

test("rejects partial signing and notarization groups before spawning", async (t) => {
  const directory = await desktopFixture();
  t.after(() => rm(directory, { recursive: true, force: true }));
  assert.throws(() => configuration(directory, { APPLE_CERTIFICATE: "cert" }), /SIGNING_IDENTITY/);
  assert.throws(() => configuration(directory, { APPLE_CERTIFICATE_PASSWORD: "password" }), /requires APPLE_CERTIFICATE/);
  assert.throws(() => configuration(directory, { APPLE_CERTIFICATE: "cert", APPLE_SIGNING_IDENTITY: "-" }), /cannot be used/);
  assert.throws(() => configuration(directory, {
    APPLE_CERTIFICATE: "cert",
    APPLE_SIGNING_IDENTITY: "Developer ID Application: Selara",
    APPLE_ID: "user@example.test",
  }), /must be set together/);
  assert.doesNotThrow(() => configuration(directory, { APPLE_SIGNING_IDENTITY: "-" }));
});

test("CI rejects missing updater key and passes enabled updater credentials verbatim", async (t) => {
  const missing = await desktopFixture({ artifacts: true, pubkey: "real-public-key" });
  t.after(() => rm(missing, { recursive: true, force: true }));
  assert.throws(() => configuration(missing, { SELARA_BUILD_MODE: "ci", SELARA_UPDATER_PUBLIC_KEY: "test-public-key" }), /TAURI_SIGNING_PRIVATE_KEY is missing/);
  const result = configuration(missing, {
    SELARA_BUILD_MODE: "ci",
    SELARA_UPDATER_PUBLIC_KEY: "test-public-key",
    TAURI_SIGNING_PRIVATE_KEY: "private-key",
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "  key password  ",
  });
  assert.equal(result.environment.TAURI_SIGNING_PRIVATE_KEY, "private-key");
  assert.equal(result.environment.TAURI_SIGNING_PRIVATE_KEY_PASSWORD, "  key password  ");
});

test("local builds disable production updates even when the source has a real public key", async (t) => {
  const directory = await desktopFixture({ artifacts: true, pubkey: "production-public-key" });
  t.after(() => rm(directory, { recursive: true, force: true }));
  const result = configuration(directory, {});
  assert.equal(result.overlay.bundle.createUpdaterArtifacts, false);
  assert.equal(result.overlay.plugins.updater.pubkey, "REPLACE_WITH_TAURI_UPDATER_PUBKEY");
});

test("production refuses missing Apple credentials and CI refuses the production key", async (t) => {
  const directory = await desktopFixture({ artifacts: true, pubkey: "production-public-key" });
  t.after(() => rm(directory, { recursive: true, force: true }));
  assert.throws(() => configuration(directory, { SELARA_BUILD_MODE: "release" }), /Production releases require/);
  assert.throws(() => configuration(directory, { SELARA_BUILD_MODE: "ci", TAURI_SIGNING_PRIVATE_KEY: "test" }), /test public key/);
  assert.throws(() => configuration(directory, { SELARA_BUILD_MODE: "typo" }), /Unknown build mode/);
});

test("supports legacy updater artifact mode and exact placeholder matching", async (t) => {
  const legacy = await desktopFixture({ artifacts: "v1Compatible", pubkey: "REPLACE_WITH_TAURI_UPDATER_PUBKEY" });
  t.after(() => rm(legacy, { recursive: true, force: true }));
  const legacyResult = configuration(legacy, {});
  assert.equal(legacyResult.overlay.bundle.createUpdaterArtifacts, false);
  const whitespace = await desktopFixture({ artifacts: true, pubkey: "  " });
  t.after(() => rm(whitespace, { recursive: true, force: true }));
  assert.equal(configuration(whitespace, {}).overlay.bundle.createUpdaterArtifacts, false);
  const padded = await desktopFixture({ artifacts: true, pubkey: "  REPLACE_WITH_TAURI_UPDATER_PUBKEY\n" });
  t.after(() => rm(padded, { recursive: true, force: true }));
  assert.equal(configuration(padded, {}).overlay.bundle.createUpdaterArtifacts, false);
});

test("resolves CLI, target cwd, overlay args, and propagates child status", async (t) => {
  const directory = await desktopFixture();
  t.after(() => rm(directory, { recursive: true, force: true }));
  const build = buildArguments(directory, {});
  assert.equal(build.cwd, directory);
  assert.equal(build.args.slice(0, 4).join(" "), "build --ci --bundles app,dmg");
  assert.equal(JSON.parse(build.args.at(-1)).bundle.macOS.signingIdentity, "-");
  let invocation;
  const result = await runBuild(directory, {
    spawnProcess(...args) {
      invocation = args;
      const child = {
        once(event, callback) {
          if (event === "close") queueMicrotask(() => callback(17, null));
          return child;
        },
      };
      return child;
    },
  });
  assert.equal(result.code, 17);
  assert.equal(invocation[2].cwd, directory);
  assert.equal(invocation[0], process.execPath);
});

test("omits empty Apple variables at a real child process boundary", async (t) => {
  const output = join(tmpdir(), `selara-build-env-${Date.now()}.json`);
  const directory = await desktopFixture({
    cliSource: "const fs = require('node:fs'); fs.writeFileSync(process.env.BUILD_TEST_OUTPUT, JSON.stringify({ cert: process.env.APPLE_CERTIFICATE, certPassword: process.env.APPLE_CERTIFICATE_PASSWORD, identity: process.env.APPLE_SIGNING_IDENTITY }));",
  });
  t.after(async () => {
    await rm(directory, { recursive: true, force: true });
    await rm(output, { force: true });
  });
  const result = await runBuild(directory, {
    environment: { PATH: process.env.PATH, BUILD_TEST_OUTPUT: output, APPLE_CERTIFICATE: "", APPLE_CERTIFICATE_PASSWORD: "", APPLE_SIGNING_IDENTITY: "" },
  });
  assert.equal(result.code, 0);
  const { readFile } = await import("node:fs/promises");
  assert.deepEqual(JSON.parse(await readFile(output, "utf8")), {});
});
