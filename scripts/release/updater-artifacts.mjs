import { execFileSync } from "node:child_process";
import { createHash, createPublicKey, verify } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

export const UPDATER_PLACEHOLDER = "REPLACE_WITH_TAURI_UPDATER_PUBKEY";

function decodedLines(value) {
  if (typeof value !== "string" || !/^[A-Za-z0-9+/=\s]+$/.test(value)) throw new Error("Invalid updater encoding");
  return Buffer.from(value.trim(), "base64").toString("utf8").trimEnd().split(/\r?\n/);
}

// Tauri wraps the standard Minisign text format in base64. Verify both the
// payload and trusted comment using Node's Ed25519 implementation. Accept the
// same legacy Ed and prehashed ED formats as tauri-plugin-updater.
export function verifyUpdaterSignature(bytes, signature, publicKey) {
  const keyLines = decodedLines(publicKey);
  const lines = decodedLines(signature);
  const key = Buffer.from(keyLines[1] ?? "", "base64");
  const signed = Buffer.from(lines[1] ?? "", "base64");
  const global = Buffer.from(lines[3] ?? "", "base64");
  if (key.length !== 42 || signed.length !== 74 || global.length !== 64 || !lines[2]?.startsWith("trusted comment: ")) {
    throw new Error("Invalid updater key or signature format");
  }
  if (!["Ed", "ED"].includes(key.subarray(0, 2).toString()) || !key.subarray(2, 10).equals(signed.subarray(2, 10))) throw new Error("Updater signature uses a different key");
  const algorithm = signed.subarray(0, 2).toString();
  if (!["Ed", "ED"].includes(algorithm)) throw new Error("Unsupported updater signature algorithm");
  const spki = Buffer.concat([Buffer.from("302a300506032b6570032100", "hex"), key.subarray(10)]);
  const verifier = createPublicKey({ key: spki, format: "der", type: "spki" });
  const payload = algorithm === "ED" ? createHash("blake2b512").update(bytes).digest() : bytes;
  const rawSignature = signed.subarray(10);
  if (!verify(null, payload, verifier, rawSignature) || !verify(null, Buffer.concat([rawSignature, Buffer.from(lines[2].slice(17))]), verifier, global)) {
    throw new Error("Updater signature verification failed");
  }
}

export function signUpdaterArtifact(cli, file, environment = process.env) {
  try {
    execFileSync(process.execPath, [cli, "signer", "sign", file], { env: environment, stdio: "pipe" });
  } catch {
    // Never echo a subprocess error object containing secret environment data.
    throw new Error("Updater signing failed; check the private key and password");
  }
  return readFileSync(`${file}.sig`, "utf8").trim();
}

export function preflightUpdaterKey(cli, publicKey, environment) {
  const directory = mkdtempSync(join(tmpdir(), "selara-key-check-"));
  try {
    const file = join(directory, "probe");
    const bytes = Buffer.from("Selara release signing preflight\n");
    writeFileSync(file, bytes);
    verifyUpdaterSignature(bytes, signUpdaterArtifact(cli, file, environment), publicKey);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

export function updaterManifest({ version, tag, repository, archive, signature, publishedAt, notes = "" }) {
  if (!/^[\w.-]+\/[\w.-]+$/.test(repository)) throw new Error("Invalid release repository");
  if (!publishedAt || !Number.isFinite(Date.parse(publishedAt))) throw new Error("Release publication date is required");
  return {
    version,
    notes,
    pub_date: publishedAt,
    platforms: { "darwin-aarch64": { signature: signature.trim(), url: `https://github.com/${repository}/releases/download/${tag}/${archive}` } },
  };
}
