import { execFileSync } from "node:child_process";
import { readdirSync } from "node:fs";
import { join } from "node:path";

// Tauri notarizes and staples the app before building the signed DMG. The
// enclosing disk image needs its own submission and ticket before publication.
export function notarizeDmg(root, environment, run = execFileSync) {
  const credentials = ["APPLE_ID", "APPLE_PASSWORD", "APPLE_TEAM_ID"].map(name => environment[name]);
  if (credentials.every(value => !value)) return;
  if (credentials.some(value => typeof value !== "string" || !value.trim())) {
    throw new Error("DMG notarization requires APPLE_ID, APPLE_PASSWORD, and APPLE_TEAM_ID");
  }
  const directory = join(root, "target/release/bundle/dmg");
  const files = readdirSync(directory, { withFileTypes: true })
    .filter(entry => entry.isFile() && entry.name.endsWith(".dmg"));
  if (files.length !== 1) throw new Error("Expected exactly one packaged DMG for notarization");
  const dmg = join(directory, files[0].name);
  let receipt;
  console.log("Submitting the packaged DMG for Apple notarization");
  try {
    receipt = JSON.parse(run("xcrun", ["notarytool", "submit", dmg,
      "--apple-id", environment.APPLE_ID, "--password", environment.APPLE_PASSWORD,
      "--team-id", environment.APPLE_TEAM_ID, "--wait", "--timeout", "30m",
      "--output-format", "json"], { stdio: "pipe", timeout: 35 * 60_000 }).toString());
  } catch {
    // Child-process errors include their arguments, including the password.
    throw new Error("DMG notarization failed; check Apple notarization history for details");
  }
  if (receipt?.status !== "Accepted") {
    throw new Error("DMG notarization was not accepted; check Apple notarization history for details");
  }
  try {
    run("xcrun", ["stapler", "staple", dmg], { stdio: "pipe", timeout: 120_000 });
    run("xcrun", ["stapler", "validate", dmg], { stdio: "pipe", timeout: 120_000 });
  } catch {
    throw new Error("DMG notarization ticket could not be stapled and validated");
  }
  console.log("DMG notarization ticket stapled and validated");
}
