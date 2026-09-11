import assert from "node:assert/strict";
import childProcess from "node:child_process";
import { randomBytes } from "node:crypto";
import { once } from "node:events";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { syncBuiltinESMExports } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { setTimeout as delay } from "node:timers/promises";
import test from "node:test";
import { validateAppleCredentials } from "./apple-signing.mjs";
import { createSigningKeychain, deleteSigningKeychain, keychainSearchLockPath } from "./keychain-search-list.mjs";

test("macOS preflight signs with an isolated keychain and preserves existing keychains", {
  skip: process.platform !== "darwin",
}, (t) => {
  const directory = mkdtempSync(join(tmpdir(), "selara-signing-test-"));
  const exec = childProcess.execFileSync;
  const run = (program, args, options = {}) => exec(program, args, {
    stdio: "pipe", timeout: 60_000, ...options,
  });
  const searchList = () => run("security", ["list-keychains", "-d", "user"]).toString();
  const originalSearchList = searchList();
  const identity = `Selara signing test ${randomBytes(8).toString("hex")}`;
  const password = randomBytes(24).toString("hex");
  const config = join(directory, "openssl.cnf");
  const key = join(directory, "key.pem");
  const certificate = join(directory, "certificate.pem");
  const exported = join(directory, "certificate.p12");
  const createdKeychains = [];
  t.after(() => {
    t.mock.restoreAll();
    syncBuiltinESMExports();
    for (const path of createdKeychains) {
      try { run("security", ["delete-keychain", path]); } catch { /* Already removed. */ }
    }
    rmSync(directory, { recursive: true, force: true });
  });

  writeFileSync(config, [
    "[req]", "distinguished_name=dn", "x509_extensions=ext", "prompt=no",
    "[dn]", `CN=${identity}`, "[ext]", "basicConstraints=critical,CA:FALSE",
    "keyUsage=critical,digitalSignature", "extendedKeyUsage=codeSigning", "",
  ].join("\n"), { mode: 0o600 });
  // Use macOS's bundled tool: Homebrew OpenSSL 3 defaults produce a PKCS#12
  // fixture that security import rejects with a misleading password error.
  run("/usr/bin/openssl", ["req", "-new", "-x509", "-newkey", "rsa:2048", "-nodes",
    "-sha256", "-days", "1", "-config", config, "-keyout", key, "-out", certificate]);
  run("/usr/bin/openssl", ["pkcs12", "-export", "-inkey", key, "-in", certificate,
    "-out", exported, "-passout", "stdin"], { input: `${password}\n` });

  // Exercise real keychain import, key access, signing, and verification. Only
  // disable the external timestamp service for this disposable test identity.
  t.mock.method(childProcess, "execFileSync", (program, args, options) => {
    if (program === "security" && args[0] === "create-keychain") {
      createdKeychains.push(args.at(-1));
    }
    const actualArgs = program === "codesign"
      ? args.map((argument) => argument === "--timestamp" ? "--timestamp=none" : argument)
      : args;
    return exec(program, actualArgs, options);
  });
  syncBuiltinESMExports();
  const environment = {
    APPLE_CERTIFICATE: readFileSync(exported).toString("base64"),
    APPLE_CERTIFICATE_PASSWORD: password,
    APPLE_SIGNING_IDENTITY: identity,
  };
  assert.doesNotThrow(() => validateAppleCredentials(environment));
  assert.equal(searchList(), originalSearchList, "Successful signing changed existing keychains");

  assert.throws(() => validateAppleCredentials({
    ...environment, APPLE_SIGNING_IDENTITY: `${identity} missing`,
  }), /Apple signing\/notarization preflight failed/);
  assert.equal(searchList(), originalSearchList, "Failed signing changed existing keychains");
});

test("concurrent keychain registration waits for the other writer and keeps both entries", {
  skip: process.platform !== "darwin", timeout: 10_000,
}, async (t) => {
  const directory = mkdtempSync(join(tmpdir(), "selara-keychain-race-"));
  const first = join(directory, "first.keychain-db");
  const second = join(directory, "second.keychain-db");
  const run = (args) => childProcess.execFileSync("/usr/bin/security", args, { stdio: "pipe" }).toString();
  const original = run(["list-keychains", "-d", "user"]);
  let holder, registrar, holderDone, registrarDone;
  t.after(async () => {
    holder?.kill();
    if (holderDone) await holderDone;
    if (registrarDone) await registrarDone;
    for (const keychain of [first, second]) {
      try { deleteSigningKeychain(keychain); } catch { /* May not have been created. */ }
    }
    rmSync(directory, { recursive: true, force: true });
    assert.equal(run(["list-keychains", "-d", "user"]), original);
  });
  createSigningKeychain(first, randomBytes(24).toString("hex"));
  createSigningKeychain(second, randomBytes(24).toString("hex"));
  const holderScript = `
    import { execFileSync } from 'node:child_process';
    import { createInterface } from 'node:readline';
    const input = createInterface({ input: process.stdin })[Symbol.asyncIterator]();
    const run = (args) => execFileSync('/usr/bin/security', args, { stdio: 'pipe' }).toString();
    const previous = [...run(['list-keychains', '-d', 'user']).matchAll(/"([^"]+)"/g)].map(m => m[1]);
    console.log('locked');
    await input.next();
    run(['list-keychains', '-d', 'user', '-s', ...previous, process.argv[1]]);
    console.log('added');
    await input.next();
    process.exit(0);
  `;
  holder = childProcess.spawn("/usr/bin/lockf", ["-k", "-t", "5", keychainSearchLockPath,
    process.execPath, "--input-type=module", "-e", holderScript, second], { stdio: ["pipe", "pipe", "pipe"] });
  holderDone = once(holder, "close");
  const holderLines = createInterface({ input: holder.stdout })[Symbol.asyncIterator]();
  assert.equal((await holderLines.next()).value, "locked");

  const registerScript = `
    import { registerSigningKeychain } from ${JSON.stringify(new URL("./keychain-search-list.mjs", import.meta.url).href)};
    console.log('starting');
    registerSigningKeychain(process.argv[1]);
  `;
  registrar = childProcess.spawn(process.execPath, ["--input-type=module", "-e", registerScript, first], {
    stdio: ["ignore", "pipe", "pipe"],
  });
  let registrationFinished = false;
  registrarDone = once(registrar, "close").then(result => { registrationFinished = true; return result; });
  const registrarLines = createInterface({ input: registrar.stdout })[Symbol.asyncIterator]();
  assert.equal((await registrarLines.next()).value, "starting");
  await delay(200);
  assert.equal(registrationFinished, false, "Registration bypassed the other writer's lock");
  holder.stdin.write("add\n");
  assert.equal((await holderLines.next()).value, "added");
  holder.stdin.write("release\n");
  assert.equal((await holderDone)[0], 0);
  assert.equal((await registrarDone)[0], 0);
  const finalList = run(["list-keychains", "-d", "user"]);
  assert(finalList.includes(first), "Concurrent registration lost the first keychain");
  assert(finalList.includes(second), "Concurrent registration lost the second keychain");
});
