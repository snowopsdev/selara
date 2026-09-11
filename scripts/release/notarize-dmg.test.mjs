import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { notarizeDmg } from "./notarize-dmg.mjs";

const credentials = {
  APPLE_ID: "notary@example.test",
  APPLE_PASSWORD: "  synthetic-secret-password  ",
  APPLE_TEAM_ID: "TESTTEAM",
};

function fixture(t, names = ["Selara_0.5.0_aarch64.dmg"]) {
  const root = mkdtempSync(join(tmpdir(), "selara-notarize-dmg-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const directory = join(root, "target/release/bundle/dmg");
  mkdirSync(directory, { recursive: true });
  for (const name of names) writeFileSync(join(directory, name), "synthetic DMG");
  return { root, dmg: join(directory, names[0] || "unused.dmg") };
}

test("unsigned local and CI builds do not access notarization tools or artifacts", () => {
  notarizeDmg("/missing-source", {}, () => assert.fail("Unexpected notarization"));
});

test("partial notarization credentials fail before any submission", () => {
  for (const missing of Object.keys(credentials)) {
    assert.throws(() => notarizeDmg("/missing-source", { ...credentials, [missing]: "" },
      () => assert.fail("Unexpected notarization")), /requires APPLE_ID, APPLE_PASSWORD, and APPLE_TEAM_ID/);
  }
});

test("requires one unambiguous DMG before submitting", t => {
  for (const names of [[], ["first.dmg", "stale.dmg"]]) {
    const f = fixture(t, names);
    assert.throws(() => notarizeDmg(f.root, credentials,
      () => assert.fail("Unexpected notarization")), /exactly one packaged DMG/);
  }
});

test("submits the final DMG and staples and validates only after Apple accepts it", t => {
  const f = fixture(t);
  const calls = [];
  notarizeDmg(f.root, credentials, (program, args, options) => {
    assert.equal(program, "xcrun");
    assert.equal(options.stdio, "pipe");
    assert(options.timeout > 0);
    calls.push(args);
    return Buffer.from(JSON.stringify({ status: "Accepted" }));
  });
  assert.deepEqual(calls, [
    ["notarytool", "submit", f.dmg, "--apple-id", credentials.APPLE_ID,
      "--password", credentials.APPLE_PASSWORD, "--team-id", credentials.APPLE_TEAM_ID,
      "--wait", "--timeout", "30m", "--output-format", "json"],
    ["stapler", "staple", f.dmg],
    ["stapler", "validate", f.dmg],
  ]);
});

test("a successful process exit without an Accepted receipt never permits stapling", t => {
  const f = fixture(t);
  for (const receipt of [{ status: "Invalid" }, { status: "In Progress" }, {}, null, "malformed"]) {
    let calls = 0;
    assert.throws(() => notarizeDmg(f.root, credentials, () => {
      calls++;
      return receipt === "malformed" ? "not JSON" : JSON.stringify(receipt);
    }), /DMG notarization (was not accepted|failed)/);
    assert.equal(calls, 1, "Attempted to staple an unaccepted submission");
  }
});

test("submission, stapling, and validation failures stop the sequence and redact subprocess details", t => {
  const f = fixture(t);
  for (const failedCall of [1, 2, 3]) {
    let calls = 0;
    assert.throws(() => notarizeDmg(f.root, credentials, () => {
      if (++calls === failedCall) {
        const error = new Error(`Command failed: --password ${credentials.APPLE_PASSWORD}`);
        error.stderr = Buffer.from(credentials.APPLE_PASSWORD);
        error.code = "ETIMEDOUT";
        throw error;
      }
      return JSON.stringify({ status: "Accepted" });
    }), error => {
      assert.match(error.message, /DMG notarization/);
      assert(!error.stack.includes(credentials.APPLE_PASSWORD));
      assert.equal(error.cause, undefined);
      return true;
    });
    assert.equal(calls, failedCall, "Continued after notarization or ticket failure");
  }
});
