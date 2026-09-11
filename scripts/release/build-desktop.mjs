import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { preflightUpdaterKey, UPDATER_PLACEHOLDER } from "./updater-artifacts.mjs";
import { validateAppleCredentials } from "./apple-signing.mjs";

const helperRoot = resolve(fileURLToPath(new URL(".", import.meta.url)), "../..");
const supportedEnvironment = [
  "APPLE_CERTIFICATE",
  "APPLE_CERTIFICATE_PASSWORD",
  "APPLE_SIGNING_IDENTITY",
  "APPLE_ID",
  "APPLE_PASSWORD",
  "APPLE_TEAM_ID",
  "TAURI_SIGNING_PRIVATE_KEY",
  "TAURI_SIGNING_PRIVATE_KEY_PASSWORD",
];

const value = (environment, name) => {
  const raw = environment[name];
  return typeof raw === "string" ? raw : "";
};
const meaningful = (environment, name) => value(environment, name).trim() !== "";

function configuration(desktopDirectory, environment = process.env) {
  const mode = environment.SELARA_BUILD_MODE || "local";
  if (!["local", "ci", "release", "recovery"].includes(mode)) throw new Error(`Unknown build mode: ${mode}`);
  const certificate = meaningful(environment, "APPLE_CERTIFICATE");
  const identity = value(environment, "APPLE_SIGNING_IDENTITY").trim();
  const password = value(environment, "APPLE_CERTIFICATE_PASSWORD");

  if (!certificate && password !== "") {
    throw new Error("APPLE_CERTIFICATE_PASSWORD requires APPLE_CERTIFICATE");
  }
  if (certificate && identity === "-") {
    throw new Error("APPLE_SIGNING_IDENTITY '-' cannot be used with APPLE_CERTIFICATE");
  }
  if (certificate) {
    if (!identity) throw new Error("APPLE_CERTIFICATE requires APPLE_SIGNING_IDENTITY");
  } else if (!certificate && identity && identity !== "-") {
    throw new Error("APPLE_SIGNING_IDENTITY requires APPLE_CERTIFICATE");
  }

  const notarizationNames = ["APPLE_ID", "APPLE_PASSWORD", "APPLE_TEAM_ID"];
  const notarization = notarizationNames.map((name) => meaningful(environment, name));
  if (notarization.some(Boolean) && !notarization.every(Boolean)) {
    throw new Error("APPLE_ID, APPLE_PASSWORD, and APPLE_TEAM_ID must be set together");
  }
  if (notarization.some(Boolean) && (!certificate || !identity || identity === "-")) {
    throw new Error("Apple notarization requires a real signing certificate");
  }

  const tauriConfigPath = resolve(desktopDirectory, "src-tauri", "tauri.conf.json");
  const baseConfig = JSON.parse(readFileSync(tauriConfigPath, "utf8"));
  const sourcePubkey = baseConfig.plugins?.updater?.pubkey;
  const configuredArtifacts = baseConfig.bundle?.createUpdaterArtifacts ?? false;
  const placeholder = typeof sourcePubkey !== "string" || sourcePubkey.trim() === "" || sourcePubkey.trim() === UPDATER_PLACEHOLDER;
  const production = mode === "release" || (mode === "recovery" && !placeholder);
  if (production && (!certificate || !identity.startsWith("Developer ID Application:") || !notarization.every(Boolean))) {
    throw new Error("Production releases require Developer ID signing and complete notarization credentials");
  }
  if (production && (placeholder || configuredArtifacts === false)) throw new Error("Production releases require an enabled updater with a real public key");
  const testPubkey = value(environment, "SELARA_UPDATER_PUBLIC_KEY").trim();
  if (mode === "ci" && (!testPubkey || testPubkey === sourcePubkey?.trim() || testPubkey === UPDATER_PLACEHOLDER)) {
    throw new Error("CI requires a separate temporary test public key (SELARA_UPDATER_PUBLIC_KEY)");
  }
  const artifacts = production || mode === "ci";
  const pubkey = mode === "ci" ? testPubkey : production ? sourcePubkey : UPDATER_PLACEHOLDER;
  if (artifacts && !meaningful(environment, "TAURI_SIGNING_PRIVATE_KEY")) {
    throw new Error("Updater artifacts are enabled but TAURI_SIGNING_PRIVATE_KEY is missing");
  }

  const normalizedEnvironment = { ...environment };
  for (const name of supportedEnvironment) delete normalizedEnvironment[name];
  if (certificate) {
    normalizedEnvironment.APPLE_CERTIFICATE = value(environment, "APPLE_CERTIFICATE");
    normalizedEnvironment.APPLE_CERTIFICATE_PASSWORD = password;
    if (identity && identity !== "-") normalizedEnvironment.APPLE_SIGNING_IDENTITY = identity;
  }
  if (notarization.every(Boolean)) {
    for (const name of notarizationNames) normalizedEnvironment[name] = value(environment, name);
  }
  if (artifacts) {
    normalizedEnvironment.TAURI_SIGNING_PRIVATE_KEY = value(environment, "TAURI_SIGNING_PRIVATE_KEY");
    normalizedEnvironment.TAURI_SIGNING_PRIVATE_KEY_PASSWORD = value(environment, "TAURI_SIGNING_PRIVATE_KEY_PASSWORD");
  }

  return {
    cwd: resolve(desktopDirectory),
    mode,
    artifacts,
    pubkey,
    environment: normalizedEnvironment,
    overlay: {
      bundle: {
        macOS: { signingIdentity: certificate && identity !== "-" ? identity : "-" },
        createUpdaterArtifacts: artifacts ? true : false,
      },
      plugins: { updater: { pubkey } },
    },
  };
}

export function buildArguments(desktopDirectory, environment = process.env) {
  const settings = configuration(desktopDirectory, environment);
  const cli = resolve(settings.cwd, "node_modules", "@tauri-apps", "cli", "tauri.js");
  if (!existsSync(cli)) throw new Error(`Tauri CLI not found in ${settings.cwd}`);
  return {
    cli,
    cwd: settings.cwd,
    env: settings.environment,
    args: ["build", "--ci", "--bundles", "app,dmg", "--config", JSON.stringify(settings.overlay)],
    overlay: settings.overlay,
    artifacts: settings.artifacts,
    pubkey: settings.pubkey,
  };
}

export function runBuild(desktopDirectory, { environment = process.env, spawnProcess = spawn } = {}) {
  const build = buildArguments(desktopDirectory, environment);
  // Fail before compilation if the private key/password does not sign for the
  // public key that will be embedded in this exact build.
  if (build.artifacts) preflightUpdaterKey(build.cli, build.pubkey, build.env);
  if (build.env.APPLE_CERTIFICATE) validateAppleCredentials(build.env);
  return new Promise((resolvePromise, reject) => {
    const child = spawnProcess(process.execPath, [build.cli, ...build.args], {
      cwd: build.cwd,
      env: build.env,
      stdio: "inherit",
    });
    child.once("error", reject);
    child.once("close", (code, signal) => resolvePromise({ code: code ?? 1, signal, build }));
  });
}

export { configuration, supportedEnvironment };

if (process.argv[1] && resolve(process.argv[1]) === resolve(fileURLToPath(import.meta.url))) {
  const desktopDirectory = process.argv[2] ? resolve(process.argv[2]) : resolve(helperRoot, "apps/selara-desktop");
  runBuild(desktopDirectory).then(({ code, signal }) => {
    if (signal) console.error(`Tauri build terminated by ${signal}`);
    process.exitCode = code;
  }).catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
