import { readFileSync, readdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { execFileSync } from "node:child_process";
import { verifyUpdaterSignature } from "./updater-artifacts.mjs";
const root = resolve(process.argv[2] || ".");
const directory = join(root, "target/release/bundle/macos");
const archives = readdirSync(directory).filter(name => name.endsWith(".app.tar.gz"));
if (archives.length !== 1) throw new Error("Expected one updater app archive");
const archive = join(directory, archives[0]);
const key = process.env.SELARA_UPDATER_PUBLIC_KEY || JSON.parse(readFileSync(join(root, "apps/selara-desktop/src-tauri/tauri.conf.json"))).plugins.updater.pubkey;
verifyUpdaterSignature(readFileSync(archive), readFileSync(`${archive}.sig`, "utf8"), key);
const entries = execFileSync("tar", ["-tzf", archive], { encoding: "utf8" });
for (const binary of ["selara-desktop", "selara", "selara-codex"]) {
  if (!entries.split("\n").includes(`Selara.app/Contents/MacOS/${binary}`)) throw new Error(`Missing updater executable: ${binary}`);
}
console.log("Verified updater archive signature and all three bundled executables");
