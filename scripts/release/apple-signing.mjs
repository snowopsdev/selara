import { execFileSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { copyFileSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

// Validate actual private-key access and notarization authentication before a
// long build. Never print subprocess arguments containing secret passwords.
export function validateAppleCredentials(environment) {
  const directory = mkdtempSync(join(tmpdir(), "selara-apple-preflight-"));
  const keychain = join(directory, "preflight.keychain-db");
  const certificate = join(directory, "certificate.p12");
  const password = randomBytes(32).toString("hex");
  const run = (program, args) => execFileSync(program, args, { stdio: "pipe", timeout: 60_000 });
  try {
    writeFileSync(certificate, Buffer.from(environment.APPLE_CERTIFICATE, "base64"), { mode: 0o600 });
    run("security", ["create-keychain", "-p", password, keychain]);
    run("security", ["unlock-keychain", "-p", password, keychain]);
    run("security", ["import", certificate, "-k", keychain, "-P", environment.APPLE_CERTIFICATE_PASSWORD, "-T", "/usr/bin/codesign"]);
    run("security", ["set-key-partition-list", "-S", "apple-tool:,apple:,codesign:", "-s", "-k", password, keychain]);
    const probe = join(directory, "probe");
    copyFileSync("/usr/bin/true", probe);
    run("codesign", ["--force", "--timestamp", "--options", "runtime", "--sign", environment.APPLE_SIGNING_IDENTITY, "--keychain", keychain, probe]);
    run("codesign", ["--verify", "--strict", probe]);
    if (environment.APPLE_ID) run("xcrun", ["notarytool", "history", "--apple-id", environment.APPLE_ID, "--password", environment.APPLE_PASSWORD, "--team-id", environment.APPLE_TEAM_ID, "--output-format", "json"]);
  } catch { throw new Error("Apple signing/notarization preflight failed; check membership, certificate with private key, and notarization credentials"); }
  finally {
    try { run("security", ["delete-keychain", keychain]); } catch { /* May not have been created. */ }
    rmSync(directory, { recursive: true, force: true });
  }
}
