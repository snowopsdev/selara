import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

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
  const pubkey = baseConfig.plugins?.updater?.pubkey;
  const configuredArtifacts = baseConfig.bundle?.createUpdaterArtifacts ?? false;
  const artifacts = configuredArtifacts !== false;
  const placeholder = typeof pubkey !== "string" || pubkey.trim() === "" || pubkey.trim() === "REPLACE_WITH_TAURI_UPDATER_PUBKEY";
  if (artifacts && !placeholder && !meaningful(environment, "TAURI_SIGNING_PRIVATE_KEY")) {
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
  if (artifacts && !placeholder) {
    normalizedEnvironment.TAURI_SIGNING_PRIVATE_KEY = value(environment, "TAURI_SIGNING_PRIVATE_KEY");
    normalizedEnvironment.TAURI_SIGNING_PRIVATE_KEY_PASSWORD = value(environment, "TAURI_SIGNING_PRIVATE_KEY_PASSWORD");
  }

  return {
    cwd: resolve(desktopDirectory),
    environment: normalizedEnvironment,
    overlay: {
      bundle: {
        macOS: { signingIdentity: certificate && identity !== "-" ? identity : "-" },
        // Preserve explicitly enabled updater signing; placeholder builds cannot update.
        createUpdaterArtifacts: artifacts && !placeholder ? configuredArtifacts : false,
      },
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
  };
}

export function runBuild(desktopDirectory, { environment = process.env, spawnProcess = spawn } = {}) {
  const build = buildArguments(desktopDirectory, environment);
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
