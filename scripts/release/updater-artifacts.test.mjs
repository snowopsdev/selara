import assert from "node:assert/strict";
import { signingFixture } from "./signing-fixture.mjs";
import test from "node:test";
import { updaterManifest, verifyUpdaterSignature } from "./updater-artifacts.mjs";


test("verifies modern and legacy Tauri signatures and rejects payload, key, and comment tampering", () => {
  for (const prehashed of [true, false]) {
    const bytes = Buffer.from("signed app archive");
    const fixture = signingFixture(bytes, prehashed);
    assert.doesNotThrow(() => verifyUpdaterSignature(bytes, fixture.signature, fixture.publicKey));
    assert.throws(() => verifyUpdaterSignature(Buffer.from("tampered"), fixture.signature, fixture.publicKey), /verification failed/);
    const other = signingFixture(bytes);
    assert.throws(() => verifyUpdaterSignature(bytes, fixture.signature, other.publicKey), /different key/);
    const changed = Buffer.from(Buffer.from(fixture.signature, "base64").toString().replace("timestamp:1", "timestamp:2")).toString("base64");
    assert.throws(() => verifyUpdaterSignature(bytes, changed, fixture.publicKey), /verification failed/);
    assert.throws(() => verifyUpdaterSignature(bytes, "malformed", fixture.publicKey), /format/);
  }
});

test("manifest uses stable release metadata, exact platform, and version-specific URLs", () => {
  const manifest = updaterManifest({ version: "1.2.3", tag: "v1.2.3", repository: "owner/repo", archive: "Selara-1.2.3-macos-arm64.app.tar.gz", signature: "sig\n", publishedAt: "2026-09-10T12:00:00Z", notes: "Release notes" });
  assert.equal(manifest.platforms["darwin-aarch64"].signature, "sig");
  assert.equal(manifest.platforms["darwin-aarch64"].url, "https://github.com/owner/repo/releases/download/v1.2.3/Selara-1.2.3-macos-arm64.app.tar.gz");
  assert.equal(manifest.version, "1.2.3");
});
