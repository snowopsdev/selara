import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { copyFileSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { developerIdRequirement } from "./apple-signature.mjs";

test("macOS parses the team requirement and rejects otherwise valid ad hoc code", { skip: process.platform !== "darwin" }, t => {
  const root = mkdtempSync(join(tmpdir(), "selara-signature-test-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const binary = join(root, "control");
  copyFileSync("/usr/bin/true", binary);
  execFileSync("codesign", ["--force", "--sign", "-", binary], { stdio: "pipe" });
  execFileSync("codesign", ["--verify", "--strict", binary], { stdio: "pipe" });
  const result = spawnSync("codesign", ["--verify", "--strict", "-R", developerIdRequirement("SELARATEAM"), binary], { encoding: "utf8" });
  assert.equal(result.status, 3, result.error?.message || result.stderr);
  assert.match(result.stderr, /failed to satisfy specified code requirement/);
});
