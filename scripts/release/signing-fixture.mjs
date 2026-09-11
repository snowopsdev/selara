import { createHash, generateKeyPairSync, randomBytes, sign } from "node:crypto";
export function signingFixture(bytes, prehashed = true) {
  const pair = generateKeyPairSync("ed25519");
  const keyId = randomBytes(8);
  const rawKey = pair.publicKey.export({ type: "spki", format: "der" }).subarray(-32);
  const publicKey = Buffer.from(`untrusted comment: fixture key\n${Buffer.concat([Buffer.from("Ed"), keyId, rawKey]).toString("base64")}\n`).toString("base64");
  const signBytes = (payload) => {
    const hashed = prehashed ? createHash("blake2b512").update(payload).digest() : payload;
    const sig = sign(null, hashed, pair.privateKey);
    const comment = "timestamp:1\tfile:Selara.app.tar.gz";
    const global = sign(null, Buffer.concat([sig, Buffer.from(comment)]), pair.privateKey);
    return Buffer.from(`untrusted comment: fixture signature\n${Buffer.concat([Buffer.from(prehashed ? "ED" : "Ed"), keyId, sig]).toString("base64")}\ntrusted comment: ${comment}\n${global.toString("base64")}\n`).toString("base64");
  };
  return { publicKey, signature: signBytes(bytes), signBytes };
}
