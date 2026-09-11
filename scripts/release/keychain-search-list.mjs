import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const keychainSearchLockPath = join(homedir(), "Library", "Caches", "dev.snowops.selara", "keychain-search-list.lock");
const script = fileURLToPath(import.meta.url);

function updateKeychain(request) {
  mkdirSync(dirname(keychainSearchLockPath), { recursive: true, mode: 0o700 });
  try {
    // Keep the lock file so all processes lock the same inode. lockf releases
    // its kernel lock on exit, including crashes; credentials travel on stdin.
    execFileSync("/usr/bin/lockf", ["-k", "-t", "30", keychainSearchLockPath,
      process.execPath, script, "--locked"], {
      input: JSON.stringify(request), stdio: "pipe", timeout: 65_000,
    });
  } catch { throw new Error("Unable to update the temporary signing keychain"); }
}

export const createSigningKeychain = (keychain, password) => updateKeychain({ operation: "create", keychain, password });
export const registerSigningKeychain = (keychain) => updateKeychain({ operation: "register", keychain });
export const deleteSigningKeychain = (keychain) => updateKeychain({ operation: "delete", keychain });

if (process.argv[1] && resolve(process.argv[1]) === resolve(script)) {
  try {
    if (process.argv[2] !== "--locked") throw new Error("Missing lock context");
    const request = JSON.parse(readFileSync(0, "utf8"));
    const run = (args) => execFileSync("/usr/bin/security", args, { stdio: "pipe", timeout: 15_000 });
    if (request.operation === "create") {
      run(["create-keychain", "-p", request.password, request.keychain]);
    } else if (request.operation === "register") {
      const searchList = run(["list-keychains", "-d", "user"]).toString()
        .split("\n").filter(line => line.trim()).map(line => {
          const match = /^\s*"(.*)"\s*$/.exec(line);
          if (!match) throw new Error("Invalid keychain search list");
          return match[1];
        });
      run(["list-keychains", "-d", "user", "-s", ...new Set([...searchList, request.keychain])]);
    } else if (request.operation === "delete") {
      // SecKeychainDelete removes this entry while retaining other keychains.
      run(["delete-keychain", request.keychain]);
    } else throw new Error("Unknown keychain operation");
  } catch {
    console.error("Unable to update the temporary signing keychain");
    process.exitCode = 1;
  }
}
