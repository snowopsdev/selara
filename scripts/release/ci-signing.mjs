// Test-only keys never leave this runner. Production uses a persistent secret.
import { execFileSync } from "node:child_process";
import { appendFileSync, mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
const root = resolve(process.argv[2] || ".");
const directory = mkdtempSync(join(tmpdir(), "selara-ci-key-"));
const privateFile = join(directory, "updater.key");
const cli = join(root, "apps/selara-desktop/node_modules/@tauri-apps/cli/tauri.js");
execFileSync(process.execPath, [cli, "signer", "generate", "--ci", "-w", privateFile, "-p", ""], { stdio: "pipe" });
const privateKey = readFileSync(privateFile, "utf8").trim();
const publicKey = readFileSync(`${privateFile}.pub`, "utf8").trim();
if (!process.env.GITHUB_ENV) throw new Error("This helper only exports keys inside GitHub Actions");
console.log(`::add-mask::${privateKey}`);
appendFileSync(process.env.GITHUB_ENV, `SELARA_BUILD_MODE=ci\nSELARA_UPDATER_PUBLIC_KEY=${publicKey}\nTAURI_SIGNING_PRIVATE_KEY=${privateKey}\nTAURI_SIGNING_PRIVATE_KEY_PASSWORD=\n`);
