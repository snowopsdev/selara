import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { JSDOM } from "jsdom";

const htmlPath = fileURLToPath(new URL("../index.html", import.meta.url));
const HTML = readFileSync(htmlPath, "utf8");

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

const config = (overrides = {}) => ({
  schema_version: 1,
  provider: {
    kind: "open_ai_compatible",
    base_url: "",
    model: "saved-model",
    api_key: null,
    auth: "api_key",
    codex_home: null,
    ...overrides.provider,
  },
  hotkey: "ctrl+shift+space",
  undo_hotkey: null,
  language: "en",
  excluded_apps: [],
  commands: [],
  limits: { soft_warn_chars: 8000, hard_max_chars: 100000, replace_warn_chars: 4000, secret_guard: true },
  ...overrides,
});

function makeHarness(options = {}) {
  const calls = [];
  const listeners = new Map();
  const authQueue = [...(options.authQueue || [])];
  const modelQueue = [...(options.modelQueue || [])];
  const axQueue = [...(options.axQueue || [])];
  const login = options.login || null;
  const cancelLogin = options.cancelLogin || Promise.resolve();
  const logout = options.logout || { state: "signed_out", logged_in: false, via_chatgpt: false, message: "No ChatGPT account is signed in" };
  const configResult = options.config || config();
  const configRequest = options.deferConfig ? deferred() : null;
  const defaultAuth = options.auth || { state: "signed_out", logged_in: false, via_chatgpt: false, message: "No ChatGPT account is signed in" };

  function next(queue, fallback) {
    if (!queue.length) return Promise.resolve(fallback);
    const item = queue.shift();
    return item && item.promise ? item.promise : Promise.resolve(item);
  }
  function invoke(command, args) {
    calls.push({ command, args });
    switch (command) {
      case "get_config": return configRequest ? configRequest.promise : Promise.resolve(configResult);
      case "chatgpt_auth_status": return next(authQueue, defaultAuth);
      case "chatgpt_login": return login ? login.promise : Promise.resolve({ state: "connected", logged_in: true, via_chatgpt: true, email: "user@example.test", plan: "pro", message: "ChatGPT account connected" });
      case "chatgpt_login_cancel": return cancelLogin;
      case "chatgpt_logout": return Promise.resolve(logout);
      case "list_chatgpt_models_cmd": return next(modelQueue, ["gpt-5.4-mini"]);
      case "list_provider_models_cmd": return next(modelQueue, ["provider-model"]);
      case "accessibility_status": return next(axQueue, options.ax || "unknown");
      case "api_key_source": return Promise.resolve("none");
      case "app_version": return Promise.resolve("0.4.1");
      case "update_status": return Promise.resolve(options.updateStatus || null);
      case "serve_status": return Promise.resolve({ running: false, pid: null, pidfile: "/tmp/selara.pid" });
      case "serve_supervisor_status": return Promise.resolve({ managed: false, external: false, pid: null, child_status: null, last_error: null });
      case "serve_log": return Promise.resolve([]);
      case "usage_summary": return Promise.resolve({ today: {}, last_30_days: {}, all_time: {} });
      case "history_list": return Promise.resolve([]);
      case "plugin:autostart|is_enabled": return Promise.resolve(false);
      case "save_config_section": return Promise.resolve(configResult);
      case "clear_api_key": return Promise.resolve("none");
      case "store_api_key": return Promise.resolve("keychain");
      default: return Promise.resolve(null);
    }
  }

  const dom = new JSDOM(HTML, {
    runScripts: "dangerously",
    pretendToBeVisual: true,
    beforeParse(window) {
      window.__TAURI__ = {
        core: { invoke },
        event: { listen: async (name, callback) => { listeners.set(name, callback); return () => listeners.delete(name); } },
      };
      window.confirm = () => true;
      window.open = () => null;
    },
  });
  const idle = async (turns = 3) => {
    for (let i = 0; i < turns; i += 1) await new Promise((resolve) => setTimeout(resolve, 0));
  };
  const ready = async () => {
    await idle(5);
    assert.ok(dom.window.document.querySelector("#section-models"), "models section should exist");
  };
  const close = () => dom.window.close();
  return { dom, calls, listeners, configRequest, authQueue, modelQueue, axQueue, login, idle, ready, close };
}

function value(dom, id, next) {
  const el = dom.window.document.getElementById(id);
  assert.ok(el, `#${id} should exist`);
  el.value = next;
  el.dispatchEvent(new dom.window.Event("input", { bubbles: true }));
  return el;
}

function callsFor(harness, command) {
  return harness.calls.filter((call) => call.command === command);
}

test("renders the account card before status and keeps it visible in API-key mode", async () => {
  const harness = makeHarness({ deferConfig: true, config: config({ provider: { auth: "api_key", model: "local-model" } }), auth: { state: "api_key_only", logged_in: true, via_chatgpt: false, message: "Codex is signed in with an API key." } });
  try {
    const { document } = harness.dom.window;
    assert.ok(document.querySelector("#chatgpt-account"), "account card should render before get_config resolves");
    assert.notEqual(document.querySelector("#chatgpt-fields").style.display, "none");
    assert.match(document.querySelector("#chatgpt-status-line").textContent, /Checking/);
    harness.configRequest.resolve(config({ provider: { auth: "api_key", model: "local-model" } }));
    await harness.ready();
    assert.match(document.querySelector("#chatgpt-status-line").textContent, /API key only/);
    assert.match(document.querySelector("#chatgpt-status-message").textContent, /API key/);
  } finally { harness.close(); }
});

test("preserves every provider draft field across an auth refresh", async () => {
  const refresh = deferred();
  const harness = makeHarness({
    config: config({ provider: { auth: "api_key", model: "saved", base_url: "https://old.example/v1", api_key: "old-key", codex_home: "/Users/test/.codex" } }),
    auth: { state: "signed_out", logged_in: false, via_chatgpt: false, message: "No ChatGPT account is signed in" },
    authQueue: [refresh],
  });
  try {
    await harness.ready();
    const { document } = harness.dom.window;
    const provider = document.querySelector("#provider-kind");
    provider.value = "open_router";
    provider.dispatchEvent(new harness.dom.window.Event("change", { bubbles: true }));
    const authButton = document.querySelector('[data-for="provider-auth"] .seg[data-value="chatgpt"]');
    authButton.click();
    value(harness.dom, "model-select", "draft-chat-model");
    value(harness.dom, "model", "draft-byok-model");
    value(harness.dom, "base_url", "https://custom.example/v1");
    value(harness.dom, "api_key", "draft-key");
    value(harness.dom, "codex_home", "/Users/test/shared-codex");
    document.querySelector("#chatgpt-refresh").click();
    await harness.idle();
    assert.equal(document.querySelector("#provider-kind").value, "open_router");
    assert.equal(document.querySelector("#provider-auth").value, "api_key", "unsupported provider keeps API-key auth");
    // Return to the OpenAI-compatible mode to verify the draft was retained by
    // the refresh even though the provider control itself normalizes auth.
    provider.value = "open_ai_compatible";
    provider.dispatchEvent(new harness.dom.window.Event("change", { bubbles: true }));
    await harness.idle();
    assert.equal(document.querySelector("#provider-auth").value, "api_key");
    assert.equal(document.querySelector("#model").value, "draft-byok-model");
    assert.equal(document.querySelector("#base_url").value, "https://custom.example/v1");
    assert.equal(document.querySelector("#api_key").value, "draft-key");
    assert.equal(document.querySelector("#codex_home").value, "/Users/test/shared-codex");
    refresh.resolve({ state: "signed_out", logged_in: false, via_chatgpt: false, message: "No ChatGPT account is signed in" });
    await harness.idle(5);
    assert.equal(document.querySelector("#model").value, "draft-byok-model");
    assert.equal(document.querySelector("#base_url").value, "https://custom.example/v1");
  } finally { harness.close(); }
});

test("ignores an older delayed auth response", async () => {
  const initial = { state: "signed_out", logged_in: false, via_chatgpt: false, message: "No ChatGPT account is signed in" };
  const first = deferred();
  const second = deferred();
  const harness = makeHarness({ authQueue: [initial, first, second], auth: initial });
  try {
    await harness.ready();
    const refresh = harness.dom.window.document.querySelector("#chatgpt-refresh");
    refresh.click();
    refresh.click();
    second.resolve({ state: "connected", logged_in: true, via_chatgpt: true, email: "new@example.test", plan: "pro", message: "ChatGPT account connected" });
    await harness.idle(3);
    first.resolve({ state: "signed_out", logged_in: false, via_chatgpt: false, message: "stale response" });
    await harness.idle(5);
    assert.match(harness.dom.window.document.querySelector("#chatgpt-status-line").textContent, /Connected/);
    assert.doesNotMatch(harness.dom.window.document.querySelector("#chatgpt-status-message").textContent, /stale response/);
  } finally { harness.close(); }
});

test("shows Cancel during login and ignores completion after cancellation", async () => {
  const login = deferred();
  const cancel = deferred();
  const harness = makeHarness({ auth: { state: "signed_out", logged_in: false, via_chatgpt: false, message: "No ChatGPT account is signed in" }, login, cancelLogin: cancel.promise });
  try {
    await harness.ready();
    const document = harness.dom.window.document;
    document.querySelector("#chatgpt-login").click();
    await harness.idle();
    assert.ok(document.querySelector("#chatgpt-login-cancel"));
    assert.ok(document.querySelector("#chatgpt-refresh"), "refresh remains responsive while login is pending");
    document.querySelector("#chatgpt-login-cancel").click();
    assert.equal(callsFor(harness, "chatgpt_login_cancel").length, 1);
    cancel.resolve();
    await harness.idle(4);
    login.resolve({ state: "connected", logged_in: true, via_chatgpt: true, email: "late@example.test", plan: "pro", message: "late login" });
    await harness.idle(5);
    assert.ok(document.querySelector("#chatgpt-login"), "cancel returns to the sign-in action");
    assert.match(document.querySelector("#chatgpt-status-line").textContent, /Signed out/);
    assert.doesNotMatch(document.querySelector("#chatgpt-status-message").textContent, /late login/);
  } finally { harness.close(); }
});

test("renders API-key-only account state without a ChatGPT email", async () => {
  const harness = makeHarness({ auth: { state: "api_key_only", logged_in: true, via_chatgpt: false, plan: null, message: "Codex is signed in with an API key." } });
  try {
    await harness.ready();
    const document = harness.dom.window.document;
    assert.match(document.querySelector("#chatgpt-status-line").textContent, /API key only/);
    assert.equal(document.querySelector("#chatgpt-email-row"), null);
    assert.match(document.querySelector("#chatgpt-status-details").textContent, /Via ChatGPTNo/);
  } finally { harness.close(); }
});

test("requires confirmation before changing the shared Codex account", async () => {
  const harness = makeHarness({
    config: config({ provider: { auth: "chatgpt", model: "saved-chat-model" } }),
    auth: { state: "connected", logged_in: true, via_chatgpt: true, email: "user@example.test", plan: "pro", message: "ChatGPT account connected" },
    logout: { state: "signed_out", logged_in: false, via_chatgpt: false, message: "No ChatGPT account is signed in" },
  });
  try {
    await harness.ready();
    const document = harness.dom.window.document;
    let confirmed = false;
    harness.dom.window.confirm = () => confirmed;
    document.querySelector("#chatgpt-logout").click();
    await harness.idle(2);
    assert.equal(callsFor(harness, "chatgpt_logout").length, 0, "cancelled sign-out should not call native logout");
    confirmed = true;
    document.querySelector("#chatgpt-logout").click();
    await harness.idle(4);
    assert.equal(callsFor(harness, "chatgpt_logout").length, 1);
    assert.match(document.querySelector("#chatgpt-status-line").textContent, /Signed out/);
  } finally { harness.close(); }
});

test("persists an optional absolute Codex home only when saving", async () => {
  const harness = makeHarness({
    config: config({ provider: { auth: "chatgpt", model: "saved-chat-model", codex_home: null } }),
    auth: { state: "connected", logged_in: true, via_chatgpt: true, email: "user@example.test", plan: "pro", message: "ChatGPT account connected" },
  });
  try {
    await harness.ready();
    value(harness.dom, "codex_home", "/Users/test/shared-codex");
    assert.equal(callsFor(harness, "save_config_section").length, 0);
    harness.dom.window.document.querySelector("#save-models").click();
    await harness.idle(4);
    const save = callsFor(harness, "save_config_section").at(-1);
    assert.equal(save.args.section, "provider");
    assert.equal(save.args.value.codex_home, "/Users/test/shared-codex");
    assert.equal(save.args.value.model, "saved-chat-model");
  } finally { harness.close(); }
});

test("model failures preserve the selection and stale model responses cannot replace it", async () => {
  const first = deferred();
  const second = deferred();
  const failure = deferred();
  const harness = makeHarness({
    config: config({ provider: { auth: "chatgpt", model: "saved-chat-model" } }),
    auth: { state: "connected", logged_in: true, via_chatgpt: true, email: "user@example.test", plan: "pro", message: "ChatGPT account connected" },
    modelQueue: [first, second, failure],
  });
  try {
    await harness.ready();
    const document = harness.dom.window.document;
    const models = document.querySelector("#chatgpt-models");
    models.click();
    models.click();
    second.resolve(["fresh-model"]);
    await harness.idle(3);
    first.resolve(["stale-model"]);
    await harness.idle(4);
    assert.equal(document.querySelector("#model-select").value, "saved-chat-model");
    assert.equal(document.querySelector('option[value="stale-model"]'), null);
    assert.ok(document.querySelector('option[value="fresh-model"]'));
    const userOption = document.createElement("option");
    userOption.value = "chosen-by-user";
    userOption.textContent = "chosen-by-user";
    document.querySelector("#model-select").append(userOption);
    document.querySelector("#model-select").value = "chosen-by-user";
    document.querySelector("#model-select").dispatchEvent(new harness.dom.window.Event("change", { bubbles: true }));
    models.click();
    failure.reject(new Error("temporary model service failure"));
    await harness.idle(5);
    assert.equal(document.querySelector("#model-select").value, "chosen-by-user");
    assert.match(document.querySelector("#chatgpt-models-hint").textContent, /Could not load models/);
  } finally { harness.close(); }
});

test("displays managed, missing, and external Accessibility states", async () => {
  const harness = makeHarness({ axQueue: ["granted", "missing", "unknown"], ax: "granted" });
  try {
    await harness.ready();
    const document = harness.dom.window.document;
    assert.match(document.querySelector("#status-accessibility-state").textContent, /Granted/);
    document.querySelector("#status-refresh").click();
    await harness.idle(5);
    assert.match(document.querySelector("#status-accessibility-state").textContent, /Missing/);
    document.querySelector("#status-refresh").click();
    await harness.idle(5);
    assert.match(document.querySelector("#status-accessibility-state").textContent, /Unknown/);
    assert.match(document.querySelector("#status-accessibility").textContent, /Start serve from this Settings app/);
  } finally { harness.close(); }
});

test("renders updater download progress and DMG fallback on error", async () => {
  const harness = makeHarness({ updateStatus: { state: "downloading", version: "0.5.0", downloaded: 512, total: 1024 } });
  try {
    await harness.ready();
    const document = harness.dom.window.document;
    assert.match(document.querySelector("#status-updates-state").textContent, /Downloading/);
    assert.equal(document.querySelector("#status-update-progress").value, 512);
    const updateListener = harness.listeners.get("update-changed");
    assert.equal(typeof updateListener, "function");
    await updateListener({ payload: { state: "error", message: "signature verification failed" } });
    assert.match(document.querySelector("#status-updates").textContent, /Retry/);
    assert.match(document.querySelector("#status-updates").textContent, /install the DMG/);
  } finally { harness.close(); }
});
