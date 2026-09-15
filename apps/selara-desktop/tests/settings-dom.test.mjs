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
  schema_version: 2,
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
  const saveQueue = [...(options.saveQueue || [])];
  const importQueue = [...(options.importQueue || [])];
  const storeQueue = [...(options.storeQueue || [])];
  const updateQueue = [...(options.updateQueue || [])];
  const usageQueue = [...(options.usageQueue || [])];
  const clearUsageQueue = [...(options.clearUsageQueue || [])];
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
      case "cli_provider_status": return options.cliStatus ? options.cliStatus(args) : Promise.resolve({ installed: true, version: "1.2.3", binary: "/usr/local/bin/cli", message: "CLI available. Sign in through the CLI before writing." });
      case "bundled_codex_version": return Promise.resolve(options.runtimeVersion || "0.153.4");
      case "update_status": return Promise.resolve(options.updateStatus || null);
      case "check_for_updates": return next(updateQueue, { state: "up_to_date" });
      case "install_update": return options.installUpdate ? options.installUpdate.promise : Promise.resolve();
      case "serve_status": return Promise.resolve({ running: false, pid: null, pidfile: "/tmp/selara.pid" });
      case "serve_supervisor_status": return Promise.resolve({ managed: false, external: false, pid: null, child_status: null, last_error: null });
      case "serve_log": return Promise.resolve([]);
      case "usage_summary": return next(usageQueue, options.usage || { today: {}, last_30_days: {}, all_time: {}, models: [] });
      case "clear_usage": return next(clearUsageQueue, { today: {}, last_30_days: {}, all_time: {}, models: [] });
      case "history_list": return Promise.resolve(options.history || []);
      case "plugin:autostart|is_enabled": return Promise.resolve(false);
      case "save_config_section": return next(saveQueue, configResult);
      case "set_shortcut_recording": return options.recordShortcut ? options.recordShortcut(args) : Promise.resolve();
      case "import_commands": return next(importQueue, null);
      case "clear_api_key": return Promise.resolve("none");
      case "store_api_key": return next(storeQueue, "keychain");
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
  return { dom, calls, listeners, configRequest, authQueue, modelQueue, axQueue, importQueue, login, idle, ready, close };
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

function isVisible(element) {
  return Boolean(element) && !element.hidden && element.getAttribute("aria-hidden") !== "true" && element.style.display !== "none";
}

function dialogFocusables(dialog) {
  return [...dialog.querySelectorAll("button, input, textarea, select, summary, [tabindex]:not([tabindex=\"-1\"])")]
    .filter((el) => {
      if (el.disabled || el.hidden || el.closest("[hidden]")) return false;
      if (el.getAttribute("aria-hidden") === "true") return false;
      const closedDetails = el.closest("details:not([open])");
      return !closedDetails || el.tagName === "SUMMARY";
    });
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

test("blocks config writes while saved settings are loading, then allows normal saves", async () => {
  const persisted = config({
    provider: { auth: "api_key", model: "persisted-model", base_url: "https://persisted.example/v1", api_key: null },
    commands: [{ id: "existing", label: "Existing", prompt: "Keep this", kind: "replace", apps: [] }],
  });
  const harness = makeHarness({ deferConfig: true, config: persisted });
  try {
    await harness.idle(3);
    const document = harness.dom.window.document;
    document.querySelector('[data-section="general"]').click();
    assert.equal(document.querySelector("#language").disabled, true);
    assert.equal(document.querySelector("#save-general").disabled, true);
    await document.querySelector("#save-general").onclick();
    document.querySelector('[data-section="models"]').click();
    assert.equal(document.querySelector("#provider-kind").disabled, true);
    assert.equal(document.querySelector("#save-models").disabled, true);
    await document.querySelector("#save-models").onclick();
    document.querySelector('[data-section="commands"]').click();
    assert.equal(document.querySelector("#cmd-new").disabled, true);
    assert.equal(document.querySelector("#cmd-import").disabled, true);
    assert.equal(callsFor(harness, "save_config_section").length, 0);

    harness.configRequest.resolve(persisted);
    await harness.idle(12);
    assert.equal(document.querySelector("#save-general").disabled, false);
    assert.equal(document.querySelector("#save-models").disabled, false);
    document.querySelector('[data-section="models"]').click();
    document.querySelector("#save-models").click();
    await harness.idle(5);
    document.querySelector('[data-section="general"]').click();
    document.querySelector("#save-general").click();
    await harness.idle(5);
    assert.ok(callsFor(harness, "save_config_section").length >= 2, "normal saves should reach the native command");
  } finally { harness.close(); }
});

test("keeps settings writes disabled after a failed config load", async () => {
  const harness = makeHarness({ deferConfig: true });
  try {
    await harness.idle(3);
    harness.configRequest.reject(new Error("config disk unavailable"));
    await harness.idle(10);
    const document = harness.dom.window.document;
    assert.match(document.querySelector("#general-config-gate").textContent, /Editing is disabled/);
    assert.equal(document.querySelector("#save-general").disabled, true);
    assert.match(document.querySelector("#models-config-gate").textContent, /could not be loaded/);
    const before = callsFor(harness, "save_config_section").length;
    await document.querySelector("#save-general").onclick();
    assert.equal(callsFor(harness, "save_config_section").length, before);
    assert.match(document.querySelector("#section-status").textContent, /Editing is disabled because saved settings could not be loaded/);
  } finally { harness.close(); }
});

test("does not let account/model refreshes seed a draft before config arrives", async () => {
  const lateModels = deferred();
  const persisted = config({
    provider: { auth: "api_key", model: "persisted-model", base_url: "https://persisted.example/v1", api_key: "saved-key", codex_home: "/Users/test/shared-codex" },
    commands: [{ id: "persisted", label: "Persisted", prompt: "Keep this", kind: "replace", apps: [] }],
  });
  const connected = { state: "connected", logged_in: true, via_chatgpt: true, email: "user@example.test", plan: "pro", message: "ChatGPT account connected" };
  const harness = makeHarness({ deferConfig: true, config: persisted, auth: connected, modelQueue: [lateModels] });
  try {
    await harness.idle(3);
    const document = harness.dom.window.document;
    document.querySelector("#chatgpt-refresh").click();
    await harness.idle(4);
    document.querySelector("#chatgpt-models").click();
    await harness.idle(3);
    assert.equal(callsFor(harness, "list_chatgpt_models_cmd").length, 1);
    harness.configRequest.resolve(persisted);
    await harness.idle(12);
    assert.equal(document.querySelector("#provider-kind").value, "open_ai_compatible");
    assert.equal(document.querySelector("#provider-auth").value, "api_key");
    assert.equal(document.querySelector("#model").value, "persisted-model");
    assert.equal(document.querySelector("#base_url").value, "https://persisted.example/v1");
    assert.equal(document.querySelector("#api_key").value, "saved-key");
    document.querySelector('[data-section="commands"]').click();
    assert.ok(document.querySelector('[data-id="persisted"]'));
    lateModels.resolve(["late-model"]);
    await harness.idle(8);
    assert.equal(document.querySelector("#model").value, "persisted-model");
    assert.equal(document.querySelector("#model-options").children.length, 0);
  } finally { harness.close(); }
});

test("preserves every provider draft field across an auth refresh and provider switch", async () => {
  const refresh = deferred();
  const initial = { state: "connected", logged_in: true, via_chatgpt: true, email: "user@example.test", plan: "pro", message: "ChatGPT account connected" };
  const harness = makeHarness({
    config: config({ provider: { auth: "api_key", model: "saved", base_url: "https://old.example/v1", api_key: "old-key", codex_home: "/Users/test/.codex" } }),
    auth: initial,
    authQueue: [initial, refresh],
  });
  try {
    await harness.ready();
    const { document } = harness.dom.window;
    let authButton = document.querySelector('[data-for="provider-auth"] .seg[data-value="chatgpt"]');
    authButton.click();
    assert.equal(document.querySelector("#model-select").disabled, false, "connected ChatGPT mode enables the subscription model selector");
    document.querySelector('[data-for="provider-auth"] .seg[data-value="api_key"]').click();
    assert.equal(document.querySelector("#model-select").disabled, true, "switching back to API-key mode disables the subscription model selector");
    authButton = document.querySelector('[data-for="provider-auth"] .seg[data-value="chatgpt"]');
    authButton.click();
    assert.equal(document.querySelector("#model-select").disabled, false);
    const chatOption = document.createElement("option");
    chatOption.value = "draft-chat-model";
    chatOption.textContent = "draft-chat-model";
    document.querySelector("#model-select").append(chatOption);
    value(harness.dom, "model-select", "draft-chat-model");
    value(harness.dom, "model", "draft-byok-model");
    value(harness.dom, "base_url", "https://custom.example/v1");
    value(harness.dom, "api_key", "draft-key");
    value(harness.dom, "codex_home", "/Users/test/shared-codex");
    document.querySelector("#chatgpt-refresh").click();
    await harness.idle();
    assert.equal(document.querySelector("#provider-kind").value, "open_ai_compatible");
    assert.equal(document.querySelector("#provider-auth").value, "chatgpt");
    assert.equal(document.querySelector("#model-select").value, "draft-chat-model");
    assert.equal(document.querySelector("#model").value, "draft-byok-model");
    assert.equal(document.querySelector("#base_url").value, "https://custom.example/v1");
    assert.equal(document.querySelector("#api_key").value, "draft-key");
    assert.equal(document.querySelector("#codex_home").value, "/Users/test/shared-codex");
    refresh.resolve({ state: "signed_out", logged_in: false, via_chatgpt: false, message: "No ChatGPT account is signed in" });
    await harness.idle(5);
    assert.equal(document.querySelector("#provider-auth").value, "chatgpt");
    assert.equal(document.querySelector("#model-select").value, "draft-chat-model");
    assert.equal(document.querySelector("#model").value, "draft-byok-model");
    assert.equal(document.querySelector("#base_url").value, "https://custom.example/v1");
    assert.equal(document.querySelector("#api_key").value, "draft-key");
    assert.equal(document.querySelector("#codex_home").value, "/Users/test/shared-codex");

    let provider = document.querySelector("#provider-kind");
    provider.value = "anthropic";
    provider.dispatchEvent(new harness.dom.window.Event("change", { bubbles: true }));
    await harness.idle(2);
    assert.equal(document.querySelector("#provider-kind").value, "anthropic");
    assert.equal(document.querySelector("#provider-auth").value, "api_key");
    assert.equal(document.querySelector("#model").value, "");
    assert.equal(document.querySelector("#base_url").value, "https://api.anthropic.com");
    assert.equal(document.querySelector("#api_key").value, "");
    assert.match(document.querySelector("#key-source-hint").textContent, /Key status describes the saved OpenAI-compatible provider/);
    assert.equal(document.querySelector("#codex_home").value, "/Users/test/shared-codex");

    provider = document.querySelector("#provider-kind");
    provider.value = "open_ai_compatible";
    provider.dispatchEvent(new harness.dom.window.Event("change", { bubbles: true }));
    await harness.idle(2);
    assert.equal(document.querySelector("#provider-kind").value, "open_ai_compatible");
    assert.equal(document.querySelector("#provider-auth").value, "chatgpt");
    assert.equal(document.querySelector("#model-select").value, "draft-chat-model");
    assert.equal(document.querySelector("#model").value, "draft-byok-model");
    assert.equal(document.querySelector("#base_url").value, "https://custom.example/v1");
    assert.equal(document.querySelector("#api_key").value, "draft-key");
    assert.equal(document.querySelector("#codex_home").value, "/Users/test/shared-codex");
  } finally { harness.close(); }
});

test("does not restore plaintext key after a successful keychain save", async () => {
  const harness = makeHarness({
    config: config({ provider: { auth: "api_key", model: "saved-model", api_key: null } }),
    auth: { state: "signed_out", logged_in: false, via_chatgpt: false, message: "No ChatGPT account is signed in" },
  });
  try {
    await harness.ready();
    value(harness.dom, "api_key", "plaintext-secret");
    harness.dom.window.document.querySelector("#save-models").click();
    await harness.idle(10);
    assert.equal(callsFor(harness, "store_api_key").length, 1);
    let provider = harness.dom.window.document.querySelector("#provider-kind");
    provider.value = "anthropic";
    provider.dispatchEvent(new harness.dom.window.Event("change", { bubbles: true }));
    await harness.idle(3);
    provider = harness.dom.window.document.querySelector("#provider-kind");
    provider.value = "open_ai_compatible";
    provider.dispatchEvent(new harness.dom.window.Event("change", { bubbles: true }));
    await harness.idle(3);
    assert.equal(harness.dom.window.document.querySelector("#api_key").value, "");
  } finally { harness.close(); }
});

test("keeps a switched provider draft while the original key save and config save complete", async () => {
  const store = deferred();
  const save = deferred();
  const postSaveAuth = deferred();
  const signedOut = { state: "signed_out", logged_in: false, via_chatgpt: false, message: "No ChatGPT account is signed in" };
  const persisted = config({ provider: { auth: "api_key", model: "openai-saved", base_url: "https://openai.example/v1", api_key: null } });
  const harness = makeHarness({ config: persisted, authQueue: [signedOut, postSaveAuth], storeQueue: [store], saveQueue: [save] });
  try {
    await harness.ready();
    const document = harness.dom.window.document;
    value(harness.dom, "model", "openai-draft");
    value(harness.dom, "api_key", "openai-secret");
    document.querySelector("#save-models").click();
    await harness.idle(2);
    assert.equal(callsFor(harness, "store_api_key").length, 1);
    assert.equal(callsFor(harness, "save_config_section").length, 0);
    assert.equal(document.querySelector("#save-models").disabled, true, "a provider save is serialized while its native writes are pending");

    let provider = document.querySelector("#provider-kind");
    provider.value = "anthropic";
    provider.dispatchEvent(new harness.dom.window.Event("change", { bubbles: true }));
    await harness.idle(2);
    value(harness.dom, "model", "anthropic-draft");
    value(harness.dom, "api_key", "anthropic-secret");
    store.resolve("keychain");
    await harness.idle(3);
    assert.equal(callsFor(harness, "save_config_section").length, 1);
    assert.equal(document.querySelector("#provider-kind").value, "anthropic");
    assert.equal(document.querySelector("#model").value, "anthropic-draft");
    assert.equal(document.querySelector("#api_key").value, "anthropic-secret");

    save.resolve(config({ provider: { auth: "api_key", model: "openai-draft", base_url: "https://openai.example/v1", api_key: null } }));
    await harness.idle(8);
    assert.equal(document.querySelector("#provider-kind").value, "anthropic");
    assert.equal(document.querySelector("#model").value, "anthropic-draft");
    assert.equal(document.querySelector("#api_key").value, "anthropic-secret");
    assert.equal(document.querySelector("#save-models").disabled, false, "a later provider save is available while account refresh is pending");
    postSaveAuth.resolve(signedOut);
    await harness.idle(5);

    provider = document.querySelector("#provider-kind");
    provider.value = "open_ai_compatible";
    provider.dispatchEvent(new harness.dom.window.Event("change", { bubbles: true }));
    await harness.idle(2);
    assert.equal(document.querySelector("#model").value, "openai-draft");
    assert.equal(document.querySelector("#api_key").value, "", "the saved OpenAI key is not restored from a draft");
  } finally { harness.close(); }
});

test("clears a submitted key while preserving same-provider edits during keychain save", async () => {
  const store = deferred();
  const save = deferred();
  const persisted = config({ provider: { auth: "api_key", model: "openai-saved", base_url: "https://openai.example/v1", api_key: null } });
  const harness = makeHarness({ config: persisted, storeQueue: [store], saveQueue: [save] });
  try {
    await harness.ready();
    const document = harness.dom.window.document;
    value(harness.dom, "model", "submitted-model");
    value(harness.dom, "api_key", "submitted-key");
    document.querySelector("#save-models").click();
    await harness.idle(2);
    value(harness.dom, "model", "new-unsaved-model");
    store.resolve("keychain");
    await harness.idle(3);
    assert.equal(document.querySelector("#api_key").value, "", "the submitted key is cleared after keychain storage");
    assert.equal(document.querySelector("#model").value, "new-unsaved-model");
    save.resolve(config({ provider: { auth: "api_key", model: "submitted-model", base_url: "https://openai.example/v1", api_key: null } }));
    await harness.idle(8);
    assert.equal(document.querySelector("#model").value, "new-unsaved-model");
    let provider = document.querySelector("#provider-kind");
    provider.value = "anthropic";
    provider.dispatchEvent(new harness.dom.window.Event("change", { bubbles: true }));
    await harness.idle(2);
    provider = document.querySelector("#provider-kind");
    provider.value = "open_ai_compatible";
    provider.dispatchEvent(new harness.dom.window.Event("change", { bubbles: true }));
    await harness.idle(2);
    assert.equal(document.querySelector("#model").value, "new-unsaved-model");
    assert.equal(document.querySelector("#api_key").value, "", "the submitted key stays cleared when returning to the provider");
  } finally { harness.close(); }
});

test("preserves a newer provider draft when the delayed config save fails", async () => {
  const store = deferred();
  const save = deferred();
  const persisted = config({ provider: { auth: "api_key", model: "openai-saved", base_url: "https://openai.example/v1", api_key: null } });
  const harness = makeHarness({ config: persisted, storeQueue: [store], saveQueue: [save] });
  try {
    await harness.ready();
    const document = harness.dom.window.document;
    value(harness.dom, "model", "openai-draft");
    value(harness.dom, "api_key", "openai-secret");
    document.querySelector("#save-models").click();
    await harness.idle(2);
    let provider = document.querySelector("#provider-kind");
    provider.value = "anthropic";
    provider.dispatchEvent(new harness.dom.window.Event("change", { bubbles: true }));
    await harness.idle(2);
    value(harness.dom, "model", "anthropic-draft");
    value(harness.dom, "api_key", "anthropic-secret");
    store.resolve("keychain");
    await harness.idle(3);
    save.reject(new Error("config write failed"));
    await harness.idle(5);
    assert.equal(document.querySelector("#provider-kind").value, "anthropic");
    assert.equal(document.querySelector("#model").value, "anthropic-draft");
    assert.equal(document.querySelector("#api_key").value, "anthropic-secret");
    assert.match(document.querySelector("#save-status").textContent, /config write failed/);
    provider = document.querySelector("#provider-kind");
    provider.value = "open_ai_compatible";
    provider.dispatchEvent(new harness.dom.window.Event("change", { bubbles: true }));
    await harness.idle(2);
    assert.equal(document.querySelector("#api_key").value, "", "a key already moved to the keychain is not restored after save failure");
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
  const harness = makeHarness({ config: config({ provider: { auth: "chatgpt", model: "saved-chat-model" } }), auth: { state: "api_key_only", logged_in: true, via_chatgpt: false, plan: null, message: "Codex is signed in with an API key." } });
  try {
    await harness.ready();
    const document = harness.dom.window.document;
    assert.match(document.querySelector("#chatgpt-status-line").textContent, /API key only/);
    assert.equal(document.querySelector("#chatgpt-email-row"), null);
    assert.match(document.querySelector("#chatgpt-status-details").textContent, /Shared account/);
    assert.match(document.querySelector("#chatgpt-status-details").textContent, /API key only/);
    assert.equal(document.querySelector("#model-select").disabled, true, "subscription model selector stays disabled without a connected ChatGPT account");
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

test("does not leave a replacement ChatGPT selector disabled after an old model load view is replaced", async () => {
  const lateModels = deferred();
  const harness = makeHarness({
    config: config({ provider: { auth: "chatgpt", model: "saved-chat-model" } }),
    auth: { state: "connected", logged_in: true, via_chatgpt: true, email: "user@example.test", plan: "pro", message: "ChatGPT account connected" },
    modelQueue: [lateModels],
  });
  try {
    await harness.ready();
    const document = harness.dom.window.document;
    document.querySelector("#chatgpt-models").click();
    await harness.idle(2);
    assert.equal(document.querySelector("#model-select").disabled, true, "the selector is disabled during its model request");
    let provider = document.querySelector("#provider-kind");
    provider.value = "anthropic";
    provider.dispatchEvent(new harness.dom.window.Event("change", { bubbles: true }));
    await harness.idle(2);
    provider = document.querySelector("#provider-kind");
    provider.value = "open_ai_compatible";
    provider.dispatchEvent(new harness.dom.window.Event("change", { bubbles: true }));
    await harness.idle(2);
    assert.equal(document.querySelector("#model-select").disabled, false, "the replacement connected selector is enabled");
    lateModels.resolve(["late-model"]);
    await harness.idle(5);
    assert.equal(document.querySelector("#model-select").disabled, false);
    assert.equal(document.querySelector('option[value="late-model"]'), null, "the old response cannot replace the new view");
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

test("uses the global shortcut for custom instructions and never saves an undo shortcut", async () => {
  const harness = makeHarness({ config: config({ undo_hotkey: "ctrl+shift+z" }) });
  try {
    await harness.ready();
    const { document } = harness.dom.window;
    assert.doesNotMatch(document.querySelector("#section-status").textContent, /Open picker|Undo last replace/);
    document.querySelector('[data-section="general"]').click();
    assert.equal(document.querySelector("#undo-hotkey"), null);
    assert.match(document.querySelector("#section-general").textContent, /Custom instruction hotkey/);
    value(harness.dom, "hotkey", "option+space");
    document.querySelector("#save-general").click();
    await harness.idle(5);
    const save = callsFor(harness, "save_config_section").at(-1);
    assert.equal(save.args.section, "general");
    assert.equal(save.args.value.language, "en");
    assert.equal(save.args.value.hotkey, "option+space");
    assert.deepEqual([...save.args.value.excluded_apps], []);
    assert.equal(Object.hasOwn(save.args.value, "undo_hotkey"), false);
  } finally { harness.close(); }
});

test("normalizes legacy command modes and saves every command as replacement", async () => {
  const persisted = config({
    schema_version: 1,
    commands: [{ id: "summary", label: "Summary", prompt: "Summarize this", kind: "popup", apps: [] }],
  });
  const harness = makeHarness({ config: persisted });
  try {
    await harness.ready();
    const { document } = harness.dom.window;
    document.querySelector('[data-section="commands"]').click();
    const row = document.querySelector('[data-id="summary"]');
    assert.ok(row);
    assert.match(document.querySelector("#section-commands").textContent, /replace(?:s)?(?: the)? selected text/i);
    assert.doesNotMatch(row.textContent, /Popup/);
    row.click();
    await harness.idle(2);
    assert.equal(document.querySelector("#cmd-kind"), null);
    assert.match(document.querySelector("#sheet-root").textContent, /Always replace the selected text/);
    value(harness.dom, "cmd-prompt", "Summarize this in one sentence");
    document.querySelector("#cmd-save").click();
    await harness.idle(5);
    const save = callsFor(harness, "save_config_section").at(-1);
    assert.equal(save.args.section, "commands");
    assert.equal(save.args.value[0].kind, "replace");
    assert.equal(save.args.value[0].prompt, "Summarize this in one sentence");
  } finally { harness.close(); }
});

test("keeps legacy history readable without picker or custom undo controls", async () => {
  const harness = makeHarness({ history: [{
    ts: 1,
    label: "Old summary",
    command_id: "summary",
    kind: "popup",
    original: "Selected text",
    result: "Summary",
    app: "Notes",
  }] });
  try {
    const { document } = harness.dom.window;
    document.querySelector('[data-section="history"]').click();
    await harness.idle(2);
    const history = document.querySelector("#section-history");
    assert.match(history.textContent, /Legacy result/);
    assert.match(history.textContent, /native undo command/);
    assert.doesNotMatch(history.textContent, /Insert below|Undo in the picker|undo_hotkey/);
  } finally { harness.close(); }
});

test("keeps a failed command save draft open and retries the same draft", async () => {
  const original = { id: "proofread", label: "Proofread", prompt: "Fix grammar", kind: "replace", apps: [] };
  const saved = { ...original, label: "Proofread gently", prompt: "Fix grammar gently" };
  const firstSave = deferred();
  const harness = makeHarness({
    config: config({ commands: [original] }),
    saveQueue: [firstSave, config({ commands: [saved] })],
  });
  try {
    await harness.ready();
    const { document } = harness.dom.window;
    document.querySelector('[data-section="commands"]').click();
    const opener = document.querySelector('[data-id="proofread"]');
    opener.focus();
    opener.click();
    await harness.idle(3);
    value(harness.dom, "cmd-label", saved.label);
    value(harness.dom, "cmd-prompt", saved.prompt);
    document.querySelector("#cmd-save").click();
    await harness.idle(3);
    assert.equal(callsFor(harness, "save_config_section").length, 1);

    firstSave.reject(new Error("commands write failed"));
    await harness.idle(6);
    assert.ok(document.querySelector("#sheet-root"), "a failed save keeps the editor open");
    assert.equal(document.querySelector("#cmd-label").value, saved.label);
    assert.equal(document.querySelector("#cmd-prompt").value, saved.prompt);
    const unchangedRow = document.querySelector('[data-id="proofread"]');
    assert.match(unchangedRow.textContent, /Proofread/);
    assert.doesNotMatch(unchangedRow.textContent, /Proofread gently/);
    const saveError = document.querySelector("#cmd-save-error");
    assert.ok(saveError, "failed command saves show an inline error");
    assert.equal(saveError.getAttribute("role"), "alert");
    assert.match(saveError.textContent, /commands write failed/);
    assert.equal(document.querySelector("#cmd-save").disabled, false, "the same Save button is available for retry");

    document.querySelector("#cmd-save").click();
    await harness.idle(8);
    assert.equal(callsFor(harness, "save_config_section").length, 2);
    assert.equal(document.querySelector("#sheet-root"), null, "a successful retry closes the editor");
    const updatedRow = document.querySelector('[data-id="proofread"]');
    assert.match(updatedRow.textContent, /Proofread gently/);
    assert.equal(document.activeElement, updatedRow, "success restores focus to the rerendered opener row");
  } finally { harness.close(); }
});

test("serializes deferred command saves so duplicate submits invoke one native write", async () => {
  const original = { id: "proofread", label: "Proofread", prompt: "Fix grammar", kind: "replace", apps: [] };
  const saved = { ...original, label: "Proofread once", prompt: "Fix grammar once" };
  const pendingSave = deferred();
  const harness = makeHarness({
    config: config({ commands: [original] }),
    saveQueue: [pendingSave],
  });
  try {
    await harness.ready();
    const { document } = harness.dom.window;
    document.querySelector('[data-section="commands"]').click();
    document.querySelector('[data-id="proofread"]').click();
    await harness.idle(3);
    value(harness.dom, "cmd-label", saved.label);
    value(harness.dom, "cmd-prompt", saved.prompt);
    const save = document.querySelector("#cmd-save");
    save.click();
    save.click();
    await harness.idle(3);
    assert.equal(callsFor(harness, "save_config_section").length, 1, "double submit is single-flight");
    assert.equal(save.disabled, true, "Save is disabled while the native write is pending");

    pendingSave.resolve(config({ commands: [saved] }));
    await harness.idle(8);
    assert.equal(callsFor(harness, "save_config_section").length, 1);
    assert.equal(document.querySelector("#sheet-root"), null);
    assert.match(document.querySelector('[data-id="proofread"]').textContent, /Proofread once/);
  } finally { harness.close(); }
});

test("failed command duplicate and delete leave the saved list unchanged", async () => {
  const original = { id: "keep", label: "Keep this", prompt: "Keep this command", kind: "replace", apps: [] };
  const duplicateSave = deferred();
  const deleteSave = deferred();
  const harness = makeHarness({
    config: config({ commands: [original] }),
    saveQueue: [duplicateSave, deleteSave],
  });
  try {
    await harness.ready();
    const { document } = harness.dom.window;
    document.querySelector('[data-section="commands"]').click();
    document.querySelector('[data-id="keep"] [data-act="dup"]').click();
    await harness.idle(3);
    assert.equal(callsFor(harness, "save_config_section").length, 1);
    assert.equal(document.querySelectorAll(".cmd-item").length, 1, "a pending duplicate is not rendered into the list");
    duplicateSave.reject(new Error("duplicate write failed"));
    await harness.idle(6);
    assert.equal(document.querySelectorAll(".cmd-item").length, 1);
    assert.ok(document.querySelector('[data-id="keep"]'));
    assert.equal(document.querySelector('[data-id^="keep-copy-"]'), null);
    let commandsError = document.querySelector("#commands-error");
    assert.ok(commandsError, "duplicate failures show a list error");
    assert.equal(commandsError.getAttribute("role"), "alert");
    assert.match(commandsError.textContent, /duplicate write failed/);

    document.querySelector('[data-id="keep"] [data-act="del"]').click();
    await harness.idle(3);
    assert.equal(callsFor(harness, "save_config_section").length, 2);
    assert.equal(document.querySelectorAll(".cmd-item").length, 1, "a pending delete keeps the row visible");
    deleteSave.reject(new Error("delete write failed"));
    await harness.idle(6);
    assert.equal(document.querySelectorAll(".cmd-item").length, 1);
    assert.ok(document.querySelector('[data-id="keep"]'));
    commandsError = document.querySelector("#commands-error");
    assert.equal(commandsError.getAttribute("role"), "alert");
    assert.match(commandsError.textContent, /delete write failed/);
  } finally { harness.close(); }
});

test("traps dialog focus, excludes collapsed advanced fields, and restores Escape or Cancel opener focus", async () => {
  const command = { id: "focus-command", label: "Focus command", prompt: "Keep focus here", kind: "replace", apps: [] };
  const harness = makeHarness({ config: config({ commands: [command] }) });
  try {
    await harness.ready();
    const { document, KeyboardEvent } = harness.dom.window;
    document.querySelector('[data-section="commands"]').click();
    const opener = document.querySelector('[data-id="focus-command"]');
    opener.focus();
    opener.click();
    await harness.idle(3);
    const app = document.querySelector("#app");
    assert.ok(app.hasAttribute("inert") || app.inert === true, "the background app is inert while editing");
    assert.equal(app.getAttribute("aria-hidden"), "true");
    const dialog = document.querySelector("#sheet-root [role=\"dialog\"]");
    assert.ok(dialog);
    const advanced = dialog.querySelector("details");
    assert.ok(advanced, "model and app controls are grouped in a disclosure");
    assert.equal(advanced.open, false, "advanced controls start collapsed");
    const focusables = dialogFocusables(dialog);
    assert.ok(focusables.length >= 4);
    assert.equal(focusables.some((el) => el.id === "cmd-model"), false, "collapsed model field is excluded from Tab order");
    assert.equal(focusables.some((el) => el.id === "cmd-apps"), false, "collapsed app field is excluded from Tab order");

    const first = focusables[0];
    const last = focusables.at(-1);
    last.focus();
    last.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true }));
    assert.equal(document.activeElement, first, "Tab wraps from the last dialog control to the first");
    first.focus();
    first.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", shiftKey: true, bubbles: true, cancelable: true }));
    assert.equal(document.activeElement, last, "Shift+Tab wraps from the first dialog control to the last");

    first.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    await harness.idle(2);
    assert.equal(document.querySelector("#sheet-root"), null);
    assert.equal(document.activeElement, document.querySelector('[data-id="focus-command"]'), "Escape restores the opener row");
    assert.equal(app.hasAttribute("inert") || app.inert === true, false);
    assert.notEqual(app.getAttribute("aria-hidden"), "true");

    const openerAgain = document.querySelector('[data-id="focus-command"]');
    openerAgain.focus();
    openerAgain.click();
    await harness.idle(3);
    document.querySelector("#cmd-cancel").click();
    await harness.idle(2);
    assert.equal(document.activeElement, document.querySelector('[data-id="focus-command"]'), "Cancel restores the opener row");
  } finally { harness.close(); }
});

test("opens More command pack tools and serializes import mutations through cancel and error", async () => {
  const command = { id: "existing", label: "Existing", prompt: "Keep this", kind: "replace", apps: [] };
  const cancelledImport = deferred();
  const failedImport = deferred();
  const harness = makeHarness({
    config: config({ commands: [command] }),
    importQueue: [cancelledImport, failedImport],
  });
  try {
    await harness.ready();
    const { document } = harness.dom.window;
    document.querySelector('[data-section="commands"]').click();
    const more = document.querySelector("#cmd-more");
    const packTools = document.querySelector("#cmd-pack-tools");
    assert.equal(packTools.hidden, true);
    assert.equal(more.getAttribute("aria-expanded"), "false");
    more.click();
    assert.equal(packTools.hidden, false);
    assert.equal(more.getAttribute("aria-expanded"), "true");

    const mode = document.querySelector("#cmd-import-mode");
    mode.value = "replace";
    mode.dispatchEvent(new harness.dom.window.Event("change", { bubbles: true }));
    document.querySelector("#cmd-import").click();
    await harness.idle(3);
    const imports = callsFor(harness, "import_commands");
    assert.equal(imports.length, 1);
    assert.equal(imports[0].args.mode, "replace");
    assert.equal(document.querySelector("#cmd-import").disabled, true);
    assert.equal(document.querySelector("#cmd-import-mode").disabled, true);
    assert.equal(document.querySelector("#cmd-new").disabled, true);
    assert.equal(document.querySelector('[data-id="existing"] [data-act="dup"]').disabled, true);
    assert.equal(document.querySelector('[data-id="existing"] [data-act="del"]').disabled, true);

    document.querySelector("#cmd-import").click();
    document.querySelector("#cmd-new").click();
    document.querySelector('[data-id="existing"] [data-act="dup"]').click();
    assert.equal(callsFor(harness, "import_commands").length, 1, "a pending import blocks a second import");
    assert.equal(callsFor(harness, "save_config_section").length, 0, "a pending import blocks other command mutations");

    cancelledImport.resolve(null);
    await harness.idle(6);
    assert.equal(document.querySelector("#cmd-import").disabled, false, "cancel restores import controls");
    assert.equal(document.querySelector("#cmd-import-mode").disabled, false);
    assert.equal(document.querySelector("#cmd-new").disabled, false);
    assert.equal(document.querySelector('[data-id="existing"] [data-act="dup"]').disabled, false);
    assert.equal(document.querySelector('[data-id="existing"] [data-act="del"]').disabled, false);

    document.querySelector("#cmd-import").click();
    await harness.idle(3);
    assert.equal(callsFor(harness, "import_commands").length, 2);
    failedImport.reject(new Error("import service failed"));
    await harness.idle(6);
    assert.equal(document.querySelector("#cmd-import").disabled, false, "error restores import controls");
    assert.equal(document.querySelector("#cmd-import-mode").disabled, false);
    assert.equal(document.querySelector("#cmd-new").disabled, false);
    const error = document.querySelector("#commands-error");
    assert.equal(error.getAttribute("role"), "alert");
    assert.match(error.textContent, /import service failed/);
  } finally { harness.close(); }
});

test("does not open a command editor for Enter on nested Duplicate or Delete controls", async () => {
  const command = { id: "keyboard-row", label: "Keyboard row", prompt: "Keep this", kind: "replace", apps: [] };
  const harness = makeHarness({ config: config({ commands: [command] }) });
  try {
    await harness.ready();
    const { document, KeyboardEvent } = harness.dom.window;
    document.querySelector('[data-section="commands"]').click();
    const row = document.querySelector('[data-id="keyboard-row"]');
    for (const action of ["dup", "del"]) {
      const control = row.querySelector(`[data-act="${action}"]`);
      control.focus();
      control.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
      await harness.idle(2);
      assert.equal(document.querySelector("#sheet-root"), null, `${action} Enter should not open the editor`);
    }
    assert.equal(callsFor(harness, "save_config_section").length, 0);
  } finally { harness.close(); }
});

test("keeps the command editor draft after failed Delete and restores focus to New command on retry success", async () => {
  const command = { id: "delete-me", label: "Delete me", prompt: "Original prompt", kind: "replace", apps: [] };
  const failedDelete = deferred();
  const harness = makeHarness({
    config: config({ commands: [command] }),
    saveQueue: [failedDelete, config({ commands: [] })],
  });
  try {
    await harness.ready();
    const { document } = harness.dom.window;
    document.querySelector('[data-section="commands"]').click();
    const opener = document.querySelector('[data-id="delete-me"]');
    opener.focus();
    opener.click();
    await harness.idle(3);
    value(harness.dom, "cmd-label", "Draft kept after delete failure");
    value(harness.dom, "cmd-prompt", "Draft prompt kept after delete failure");
    document.querySelector("#cmd-delete").click();
    await harness.idle(3);
    assert.equal(callsFor(harness, "save_config_section").length, 1);
    assert.equal(document.querySelector("#cmd-delete").disabled, true);
    failedDelete.reject(new Error("delete service failed"));
    await harness.idle(6);
    assert.ok(document.querySelector("#sheet-root"), "failed Delete keeps the editor open");
    assert.equal(document.querySelector("#cmd-label").value, "Draft kept after delete failure");
    assert.equal(document.querySelector("#cmd-prompt").value, "Draft prompt kept after delete failure");
    assert.equal(document.querySelector("#cmd-delete").disabled, false);
    const saveError = document.querySelector("#cmd-save-error");
    assert.equal(saveError.getAttribute("role"), "alert");
    assert.match(saveError.textContent, /delete service failed/);

    document.querySelector("#cmd-delete").click();
    await harness.idle(3);
    assert.equal(callsFor(harness, "save_config_section").length, 2);
    const successfulDelete = harness.calls.filter((call) => call.command === "save_config_section").at(-1);
    assert.deepEqual(successfulDelete.args.value, []);
    await harness.idle(2);
    assert.equal(document.querySelector("#sheet-root"), null);
    assert.equal(document.querySelector('[data-id="delete-me"]'), null);
    assert.equal(document.activeElement, document.querySelector("#cmd-new"), "deleting the opener restores focus to New command");
  } finally { harness.close(); }
});

test("orders active connection controls before the account in API mode and reverses that order for ChatGPT", async () => {
  const connected = { state: "connected", logged_in: true, via_chatgpt: true, email: "user@example.test", plan: "pro", message: "ChatGPT account connected" };
  const harness = makeHarness({
    config: config({ provider: { auth: "api_key", model: "saved-local", base_url: "https://provider.example/v1" } }),
    auth: connected,
  });
  try {
    await harness.ready();
    const { document, Node } = harness.dom.window;
    const byokFields = document.querySelector("#byok-fields");
    const chatgptFields = document.querySelector("#chatgpt-fields");
    const accountControls = document.querySelector("#chatgpt-account-controls");
    assert.ok(byokFields);
    assert.ok(chatgptFields);
    assert.ok(accountControls instanceof harness.dom.window.HTMLDetailsElement);
    assert.equal(byokFields.compareDocumentPosition(chatgptFields) & Node.DOCUMENT_POSITION_FOLLOWING, Node.DOCUMENT_POSITION_FOLLOWING, "API connection fields precede the inactive account fields");
    assert.equal(accountControls.open, false, "account management stays collapsed in API-key mode");
    assert.equal(isVisible(document.querySelector("#chatgpt-status-line")), true, "shared account status remains visible");
    assert.equal(isVisible(document.querySelector("#model")), true);
    assert.equal(isVisible(document.querySelector("#base_url")), true);

    document.querySelector('[data-for="provider-auth"] .seg[data-value="chatgpt"]').click();
    await harness.idle(3);
    const activeChatgptFields = document.querySelector("#chatgpt-fields");
    const inactiveByokFields = document.querySelector("#byok-fields");
    const activeAccountControls = document.querySelector("#chatgpt-account-controls");
    assert.equal(activeChatgptFields.compareDocumentPosition(inactiveByokFields) & Node.DOCUMENT_POSITION_FOLLOWING, Node.DOCUMENT_POSITION_FOLLOWING, "ChatGPT account fields precede the inactive API form");
    assert.equal(activeAccountControls.open, true, "account controls open when ChatGPT mode is selected");
    assert.equal(isVisible(document.querySelector("#model-select")), true, "ChatGPT mode exposes the subscription model control");
    assert.equal(isVisible(document.querySelector("#byok-fields")), false, "ChatGPT mode hides the API form");
  } finally { harness.close(); }
});

test("keeps focus on the Base URL field across sequential edits", async () => {
  const harness = makeHarness({ config: config({ provider: { auth: "api_key", model: "saved-model", base_url: "https://saved.example/v1" } }) });
  try {
    await harness.ready();
    const { document } = harness.dom.window;
    const baseUrl = document.querySelector("#base_url");
    baseUrl.focus();
    for (const draft of ["h", "https://draft.example", "https://draft.example/v1"]) {
      baseUrl.value = draft;
      baseUrl.dispatchEvent(new harness.dom.window.Event("input", { bubbles: true }));
      assert.equal(document.activeElement, baseUrl, `Base URL retains focus after editing ${draft}`);
    }
  } finally { harness.close(); }
});

test("preserves provider drafts, keychain preference, and advanced disclosure through account refresh", async () => {
  const initial = { state: "connected", logged_in: true, via_chatgpt: true, email: "user@example.test", plan: "pro", message: "ChatGPT account connected" };
  const refreshed = deferred();
  const harness = makeHarness({
    config: config({ provider: { auth: "api_key", model: "saved-model", base_url: "https://saved.example/v1", api_key: null, codex_home: null } }),
    auth: initial,
    authQueue: [initial, refreshed],
  });
  try {
    await harness.ready();
    const { document } = harness.dom.window;
    const advanced = document.querySelector("#provider-advanced");
    assert.ok(advanced instanceof harness.dom.window.HTMLDetailsElement);
    assert.equal(advanced.open, false);
    advanced.querySelector("summary").click();
    await harness.idle();
    assert.equal(advanced.open, true);
    document.querySelector("#store-keychain").click();
    assert.equal(document.querySelector("#store-keychain").checked, false);
    value(harness.dom, "model", "draft-model");
    value(harness.dom, "base_url", "https://draft.example/v1");
    value(harness.dom, "api_key", "draft-key");
    value(harness.dom, "codex_home", "/Users/test/draft-codex");

    document.querySelector("#chatgpt-refresh").click();
    await harness.idle(3);
    assert.equal(document.querySelector("#provider-advanced").open, true, "open advanced disclosure survives a pending account refresh");
    assert.equal(document.querySelector("#store-keychain").checked, false);
    assert.equal(document.querySelector("#model").value, "draft-model");
    assert.equal(document.querySelector("#base_url").value, "https://draft.example/v1");
    assert.equal(document.querySelector("#api_key").value, "draft-key");
    assert.equal(document.querySelector("#codex_home").value, "/Users/test/draft-codex");

    refreshed.resolve({ state: "signed_out", logged_in: false, via_chatgpt: false, message: "No ChatGPT account is signed in" });
    await harness.idle(6);
    assert.equal(document.querySelector("#provider-advanced").open, true, "open advanced disclosure survives the completed account refresh");
    assert.equal(document.querySelector("#store-keychain").checked, false);
    assert.equal(document.querySelector("#model").value, "draft-model");
    assert.equal(document.querySelector("#base_url").value, "https://draft.example/v1");
    assert.equal(document.querySelector("#api_key").value, "draft-key");
    assert.equal(document.querySelector("#codex_home").value, "/Users/test/draft-codex");
  } finally { harness.close(); }
});

test("reveals and focuses invalid hidden Codex home validation", async () => {
  const harness = makeHarness({ config: config({ provider: { auth: "api_key", model: "saved-model", codex_home: null } }) });
  try {
    await harness.ready();
    const { document } = harness.dom.window;
    assert.equal(document.querySelector("#provider-advanced").open, false);
    value(harness.dom, "codex_home", "relative/codex-home");
    document.querySelector("#save-models").click();
    await harness.idle(5);
    const advanced = document.querySelector("#provider-advanced");
    const feedback = document.querySelector("#models-feedback");
    assert.equal(callsFor(harness, "save_config_section").length, 0, "invalid Codex home is rejected before the native save");
    assert.equal(advanced.open, true, "validation opens the disclosure containing the invalid field");
    assert.equal(document.activeElement.id, "codex_home", "validation focuses the invalid hidden field");
    assert.equal(feedback.getAttribute("role"), "alert");
    assert.match(feedback.textContent, /absolute/i);
  } finally { harness.close(); }
});

test("reports failed provider saves while retaining the complete draft", async () => {
  const failedSave = deferred();
  const harness = makeHarness({
    config: config({ provider: { auth: "api_key", model: "saved-model", base_url: "https://saved.example/v1", api_key: null, codex_home: null } }),
    saveQueue: [failedSave],
  });
  try {
    await harness.ready();
    const { document } = harness.dom.window;
    value(harness.dom, "model", "draft-model");
    value(harness.dom, "base_url", "https://draft.example/v1");
    value(harness.dom, "api_key", "draft-key");
    value(harness.dom, "codex_home", "/Users/test/draft-codex");
    const keychain = document.querySelector("#store-keychain");
    keychain.checked = false;
    keychain.dispatchEvent(new harness.dom.window.Event("change", { bubbles: true }));
    document.querySelector("#save-models").click();
    await harness.idle(3);
    assert.equal(callsFor(harness, "store_api_key").length, 0, "this failure exercises the config write path");
    assert.equal(callsFor(harness, "save_config_section").length, 1);
    failedSave.reject(new Error("provider write failed"));
    await harness.idle(6);
    const feedback = document.querySelector("#models-feedback");
    assert.equal(feedback.getAttribute("role"), "alert");
    assert.match(feedback.textContent, /provider write failed/);
    assert.equal(document.querySelector("#provider-auth").value, "api_key");
    assert.equal(document.querySelector("#model").value, "draft-model");
    assert.equal(document.querySelector("#base_url").value, "https://draft.example/v1");
    assert.equal(document.querySelector("#api_key").value, "draft-key");
    assert.equal(document.querySelector("#codex_home").value, "/Users/test/draft-codex");
    assert.equal(document.querySelector("#save-models").disabled, false, "failed saves leave the action available for retry");
  } finally { harness.close(); }
});


test("checks for updates from the window footer across tabs without disturbing settings drafts or feedback", async () => {
  const check = deferred();
  const harness = makeHarness({ updateStatus: { state: "up_to_date" }, updateQueue: [check] });
  try {
    await harness.ready();
    const { document } = harness.dom.window;
    const footer = document.querySelector(".app-footer");
    assert.equal(footer.parentElement.id, "app");
    assert.equal(footer.closest("aside"), null);
    assert.equal(footer.querySelector("#app-version").textContent, "v0.4.1");
    assert.equal(document.querySelector("#section-status #status-updates"), null);
    document.querySelector('[data-section="models"]').click();
    const model = value(harness.dom, "model", "unsaved-model");
    const feedback = document.querySelector("#save-status").textContent;
    footer.querySelector("#status-check-update").click();
    assert.equal(footer.querySelector("#status-check-update").disabled, true);
    assert.match(footer.querySelector("#status-updates-state").textContent, /Checking/);
    document.querySelector('[data-section="general"]').click();
    await harness.listeners.get("update-changed")({ payload: { state: "up_to_date" } });
    footer.querySelector("#status-check-update").click();
    assert.equal(callsFor(harness, "check_for_updates").length, 1);
    check.resolve({ state: "available", version: "0.5.0", notes: "A better sidebar" });
    await harness.idle();
    assert.match(footer.textContent, /v0.5.0 available/);
    assert.equal(footer.querySelector("#status-install-update").disabled, false);
    assert.match(footer.textContent, /Release notes/);
    assert.equal(document.querySelector(".section.active").id, "section-general");
    assert.equal(document.querySelector("#model"), model);
    assert.equal(model.value, "unsaved-model");
    assert.equal(document.querySelector("#save-status").textContent, feedback);
  } finally { harness.close(); }
});

test("keeps footer update actions disabled through installation events and enables retry after failure", async () => {
  const install = deferred();
  const harness = makeHarness({ updateStatus: { state: "available", version: "0.5.0" }, installUpdate: install });
  try {
    await harness.ready();
    const { document } = harness.dom.window;
    document.querySelector('[data-section="general"]').click();
    document.querySelector("#status-install-update").click();
    assert.equal(callsFor(harness, "install_update").length, 1);
    assert.equal(document.querySelector("#status-check-update").disabled, true);
    const update = harness.listeners.get("update-changed");
    await update({ payload: { state: "downloading", downloaded: 250, total: 1000 } });
    assert.equal(document.querySelector("#status-update-progress").value, 250);
    await update({ payload: { state: "waiting", version: "0.5.0" } });
    assert.match(document.querySelector("#status-update-result").textContent, /Finishing active work/);
    assert.equal(document.querySelector("#status-check-update").disabled, true);
    install.reject(new Error("signature verification failed"));
    await harness.idle();
    assert.match(document.querySelector("#status-update-result").textContent, /signature verification failed/);
    assert.equal(document.querySelector("#status-check-update").disabled, false);
    assert.equal(document.querySelector("#status-check-update").textContent, "Retry");
    const details = document.querySelector("#update-details");
    assert.equal(details.hidden, false);
    document.querySelector("#update-details-toggle").click();
    assert.equal(details.open, true);
    document.dispatchEvent(new harness.dom.window.KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    assert.equal(details.open, false);
    assert.equal(document.activeElement.id, "update-details-toggle");
    document.querySelector("#status-check-update").click();
    await harness.idle();
    assert.match(document.querySelector("#status-updates-state").textContent, /Up to date/);
    assert.equal(document.querySelector(".section.active").id, "section-general");
  } finally { harness.close(); }
});


const usageFixture = {
  today: { requests: 1, input: 100, output: 50, cost_usd: 0.01, unpriced: 0 },
  last_30_days: { requests: 3, input: 700, output: 200, cost_usd: 0.03, unpriced: 1 },
  all_time: { requests: 4, input: 1000, output: 500, cost_usd: 0.03, unpriced: 2 },
  models: [
    { kind: "anthropic", model: "model-a", requests: 1, input: 300, output: 100, cost_usd: 0.03, unpriced: 0 },
    { kind: "chatgpt_codex", model: "model-b", requests: 3, input: 700, output: 400, cost_usd: null, unpriced: 3 },
  ],
};

test("opens a dedicated Usage page with period totals and all-time model/provider detail", async () => {
  const harness = makeHarness({ usage: usageFixture });
  try {
    await harness.ready();
    const { document } = harness.dom.window;
    assert.equal(document.querySelector("#section-status #status-usage"), null);
    assert.equal(callsFor(harness, "usage_summary").length, 0, "status does not fetch usage");
    document.querySelector('[data-section="usage"]').click();
    await harness.idle();
    assert.equal(document.querySelector(".section.active").id, "section-usage");
    const rows = document.querySelectorAll("#status-usage tbody tr");
    assert.equal(rows.length, 3);
    assert.match(rows[2].textContent, /All time41,000500~\$0.03 \+2 unpriced/);
    const models = document.querySelectorAll(".usage-models tbody tr");
    assert.match(models[0].textContent, /model-bChatGPT3700400n\/a/);
    assert.match(models[1].textContent, /model-aAnthropic1300100~\$0.03/);
    assert.match(document.querySelector(".usage-breakdown").textContent, /All-time totals/);
    assert.ok(document.querySelector(".app-footer #status-check-update"));
  } finally { harness.close(); }
});

test("serializes usage refreshes, retains totals on failure, and recovers to the empty state", async () => {
  const refresh = deferred();
  const harness = makeHarness({ usageQueue: [usageFixture, refresh, { today: {}, last_30_days: {}, all_time: {}, models: [] }] });
  try {
    await harness.ready();
    const { document } = harness.dom.window;
    document.querySelector('[data-section="usage"]').click();
    await harness.idle();
    document.querySelector("#usage-refresh").click();
    document.querySelector('[data-section="status"]').click();
    document.querySelector('[data-section="usage"]').click();
    assert.equal(callsFor(harness, "usage_summary").length, 2);
    assert.equal(document.querySelector("#usage-clear").disabled, true);
    refresh.reject(new Error("Could not read usage"));
    await harness.idle();
    assert.match(document.querySelector('#section-usage [role="alert"]').textContent, /Could not read usage.*last loaded totals/);
    assert.equal(document.querySelectorAll(".usage-models tbody tr").length, 2);
    document.querySelector("#usage-refresh").click();
    await harness.idle();
    assert.equal(document.querySelector('#section-usage [role="alert"]'), null);
    assert.match(document.querySelector("#status-usage").textContent, /No usage recorded yet/);
    assert.equal(document.querySelector("#usage-clear").disabled, true);
  } finally { harness.close(); }
});

test("requires confirmation to clear usage, blocks duplicate clears, and retains totals on failure", async () => {
  const clear = deferred();
  const harness = makeHarness({ usage: usageFixture, clearUsageQueue: [clear] });
  try {
    await harness.ready();
    const { document } = harness.dom.window;
    document.querySelector('[data-section="usage"]').click();
    await harness.idle();
    harness.dom.window.confirm = () => false;
    document.querySelector("#usage-clear").click();
    assert.equal(callsFor(harness, "clear_usage").length, 0);
    harness.dom.window.confirm = () => true;
    document.querySelector("#usage-clear").click();
    document.querySelector("#usage-clear").click();
    document.querySelector("#usage-refresh").click();
    assert.equal(callsFor(harness, "clear_usage").length, 1);
    assert.equal(callsFor(harness, "usage_summary").length, 1);
    clear.reject(new Error("Could not clear usage"));
    await harness.idle();
    assert.match(document.querySelector('#section-usage [role="alert"]').textContent, /Could not clear usage/);
    assert.equal(document.querySelectorAll(".usage-models tbody tr").length, 2);
    document.querySelector("#usage-clear").click();
    await harness.idle();
    assert.match(document.querySelector("#status-usage").textContent, /No usage recorded yet/);
    assert.equal(callsFor(harness, "save_config_section").length, 0);
    assert.equal(callsFor(harness, "history_clear").length, 0);
  } finally { harness.close(); }
});


test("presents Providers with bundled Codex version and preserves drafts across connection selection", async () => {
  const harness = makeHarness({ config: config({ provider: { kind: "open_ai_compatible", auth: "api_key", model: "api-model", base_url: "https://api.example/v1" } }) });
  try {
    await harness.ready();
    const { document } = harness.dom.window;
    assert.equal(document.querySelector('[data-section="models"]').textContent.trim(), "Providers");
    assert.equal(document.querySelector("#section-models h1").textContent, "Providers");
    assert.match(document.querySelector('[data-provider-choice="codex"]').textContent, /v0.153.4.*Bundled/);
    assert.ok(document.querySelector('[data-provider-choice="openai"] .status-dot.ok'));
    document.querySelector('[data-section="models"]').click();
    value(harness.dom, "model", "unsaved-api-model");
    value(harness.dom, "base_url", "https://unsaved.example/v1");
    document.querySelector('[data-provider-choice="codex"]').click();
    assert.equal(document.querySelector("#provider-detail-title").textContent, "Codex");
    assert.equal(document.querySelector("#provider-runtime-version").textContent, "v0.153.4");
    assert.equal(document.querySelector("#provider-auth").value, "chatgpt");
    assert.equal(document.querySelector('[data-provider-choice="codex"]').getAttribute("aria-pressed"), "true");
    document.querySelector('[data-provider-choice="anthropic"]').click();
    assert.equal(document.querySelector("#provider-kind").value, "anthropic");
    assert.equal(document.querySelector("#provider-auth").value, "api_key");
    assert.equal(document.querySelector("#provider-detail-title").textContent, "Anthropic");
    assert.equal(document.querySelector("#provider-runtime-version").textContent, "API");
    value(harness.dom, "model", "claude-draft");
    document.querySelector('[data-provider-choice="openai"]').click();
    assert.equal(document.querySelector("#model").value, "unsaved-api-model");
    assert.equal(document.querySelector("#base_url").value, "https://unsaved.example/v1");
    document.querySelector('[data-provider-choice="anthropic"]').click();
    assert.equal(document.querySelector("#model").value, "claude-draft");
    document.querySelector('[data-provider-choice="openrouter"]').click();
    assert.equal(document.querySelector("#provider-kind").value, "open_router");
    assert.equal(document.querySelector("#provider-detail-title").textContent, "OpenRouter");
    assert.equal(callsFor(harness, "save_config_section").length, 0, "browsing connections does not activate them");
    assert.equal(callsFor(harness, "chatgpt_login").length, 0);
    await harness.idle();
  } finally { harness.close(); }
});

test("saves the selected provider through the existing config command and updates the active indicator", async () => {
  const saved = config({ provider: { kind: "anthropic", auth: "api_key", model: "claude-selected", base_url: "https://api.anthropic.com", api_key: null, codex_home: null } });
  const harness = makeHarness({ saveQueue: [saved] });
  try {
    await harness.ready();
    const { document } = harness.dom.window;
    document.querySelector('[data-provider-choice="anthropic"]').click();
    value(harness.dom, "model", "claude-selected");
    document.querySelector("#save-models").click();
    await harness.idle(6);
    const saves = callsFor(harness, "save_config_section");
    assert.equal(saves.length, 1);
    assert.equal(saves[0].args.section, "provider");
    assert.equal(saves[0].args.value.kind, "anthropic");
    assert.equal(saves[0].args.value.model, "claude-selected");
    assert.ok(document.querySelector('[data-provider-choice="anthropic"] .status-dot.ok'));
    assert.equal(document.querySelector('[data-provider-choice="openai"] .status-dot.ok'), null);
  } finally { harness.close(); }
});


test("restores saved CLI connections and saves executable/model without API credentials", async () => {
  const cfg = config({ provider_connections: { "claude_cli:api_key": { kind: "claude_cli", auth: "api_key", model: "sonnet", cli_binary: "/Applications/CLI Tools/claude", base_url: "" } } });
  const saved = { ...cfg, provider: cfg.provider_connections["claude_cli:api_key"] };
  const harness = makeHarness({ config: cfg, saveQueue: [saved] });
  try {
    await harness.ready();
    const { document } = harness.dom.window;
    document.querySelector('[data-provider-choice="claude"]').click();
    assert.equal(document.querySelector("#model").value, "sonnet");
    assert.equal(document.querySelector("#cli-binary").value, "/Applications/CLI Tools/claude");
    assert.equal(document.querySelector("#cli-fields").hidden, false);
    assert.equal(document.querySelector("#byok-key-wrap").hidden, true);
    assert.equal(document.querySelector("#load-models").hidden, true);
    assert.equal(document.querySelector("#chatgpt-fields").style.display, "none");
    value(harness.dom, "model", "");
    value(harness.dom, "cli-binary", "/custom/claude");
    document.querySelector('[data-provider-choice="cursor"]').click();
    assert.equal(document.querySelector("#cli-binary").value, "");
    document.querySelector('[data-provider-choice="claude"]').click();
    assert.equal(document.querySelector("#cli-binary").value, "/custom/claude");
    document.querySelector("#save-models").click();
    await harness.idle(6);
    const save = callsFor(harness, "save_config_section")[0].args.value;
    assert.equal(save.kind, "claude_cli");
    assert.equal(save.cli_binary, "/custom/claude");
    assert.equal(save.model, "", "CLI default model may be saved");
    assert.equal(save.api_key, null);
    assert.equal(callsFor(harness, "store_api_key").length, 0);
    assert.equal(callsFor(harness, "chatgpt_login").length, 0);
  } finally { harness.close(); }
});

test("keeps stale CLI inspection results separate from the selected executable", async () => {
  const pending = deferred();
  const harness = makeHarness({ cliStatus: ({ binary }) => binary === "/slow/cli" ? pending.promise : Promise.resolve({ installed: true, version: "1.2.3", message: "CLI available; account access has not been checked." }) });
  try {
    await harness.ready();
    const { document } = harness.dom.window;
    document.querySelector('[data-provider-choice="opencode"]').click();
    value(harness.dom, "cli-binary", "/slow/cli");
    document.querySelector("#cli-refresh").click();
    assert.equal(document.querySelector("#cli-refresh").disabled, true);
    value(harness.dom, "cli-binary", "/different/cli");
    assert.equal(document.querySelector("#cli-refresh").disabled, false);
    assert.equal(document.querySelector("#provider-runtime-version").textContent, "Not checked");
    pending.resolve({ installed: true, version: "old-version", message: "Old executable" });
    await harness.idle();
    assert.equal(document.querySelector("#provider-runtime-version").textContent, "Not checked");
    assert.doesNotMatch(document.querySelector("#cli-runtime-status").textContent, /Old executable/);
    document.querySelector("#cli-refresh").click();
    await harness.idle();
    assert.equal(document.querySelector("#provider-runtime-version").textContent, "1.2.3");
    assert.match(document.querySelector("#cli-runtime-status").textContent, /account access has not been checked/);
  } finally { harness.close(); }
});

test("counts requests with unavailable token metadata and labels their tokens n/a", async () => {
  const bucket = { requests: 1, input: 0, output: 0, cost_usd: null, unpriced: 1, tokens_missing: 1 };
  const harness = makeHarness({ usage: { today: bucket, last_30_days: bucket, all_time: bucket, models: [{ kind: "cursor_cli", model: "", ...bucket }] } });
  try {
    await harness.ready();
    harness.dom.window.document.querySelector('[data-section="usage"]').click();
    await harness.idle();
    const { document } = harness.dom.window;
    const row = document.querySelector(".usage-models tbody tr");
    assert.match(row.textContent, /CLI defaultCursor/);
    assert.deepEqual([...row.querySelectorAll("td")].map((td) => td.textContent), ["1", "n/a", "n/a", "n/a"]);
    assert.match(document.querySelector(".usage-note").textContent, /exclude 1 request without token metadata/);
  } finally { harness.close(); }
});

test("provider switches save independently, serialize writes, and preserve unsaved fields", async () => {
  const pending = deferred();
  const initial = config();
  const enabled = { ...initial, provider_connections: { "claude_cli:api_key": { kind: "claude_cli", enabled: true } } };
  const harness = makeHarness({ config: initial, saveQueue: [pending] });
  try {
    await harness.ready();
    const { document } = harness.dom.window;
    const toggle = (id) => document.querySelector(`[data-provider-enabled="${id}"]`);
    assert.equal(document.querySelectorAll('[role="switch"][data-provider-enabled]').length, 7);
    assert.equal(toggle("openai").getAttribute("aria-checked"), "true", "existing active connection remains enabled");
    assert.equal(toggle("claude").getAttribute("aria-checked"), "false");
    assert.equal(toggle("claude").closest("[data-provider-choice]"), null, "switch is a separate control");
    value(harness.dom, "model", "unsaved-model");
    value(harness.dom, "api_key", "unsaved-key");
    toggle("claude").click();
    assert.ok(toggle("cursor").disabled);
    assert.ok(document.querySelector("#save-models").disabled);
    toggle("cursor").click();
    document.querySelector("#save-models").click();
    assert.equal(callsFor(harness, "save_config_section").length, 1);
    const request = callsFor(harness, "save_config_section")[0].args;
    assert.equal(request.section, "provider_enabled");
    assert.deepEqual(JSON.parse(JSON.stringify(request.value)), { kind: "claude_cli", auth: "api_key", enabled: true });
    pending.resolve(enabled);
    await harness.idle();
    assert.equal(toggle("claude").getAttribute("aria-checked"), "true");
    assert.equal(document.querySelector("#model").value, "unsaved-model");
    assert.equal(document.querySelector("#api_key").value, "unsaved-key");
    assert.ok(document.querySelector('[data-provider-choice="openai"] .status-dot.ok'));
    assert.equal(document.querySelector('[data-provider-choice="claude"] .status-dot'), null);
    assert.equal(callsFor(harness, "store_api_key").length, 0);
    assert.equal(document.querySelector("#save-models").textContent, "Save and use");
  } finally { harness.close(); }
});

test("failed provider toggles retain saved state and recover after navigation", async () => {
  const pending = deferred();
  const harness = makeHarness({ saveQueue: [pending] });
  try {
    await harness.ready();
    const { document } = harness.dom.window;
    document.querySelector('[data-provider-enabled="openai"]').click();
    document.querySelector('[data-provider-choice="claude"]').click();
    value(harness.dom, "cli-binary", "/my/claude");
    assert.ok(document.querySelector('[data-provider-enabled="claude"]').disabled);
    pending.reject(new Error("Could not write config"));
    await harness.idle();
    assert.equal(document.querySelector('[data-provider-enabled="openai"]').getAttribute("aria-checked"), "true");
    assert.equal(document.querySelector("#cli-binary").value, "/my/claude");
    assert.equal(document.querySelector('[data-provider-choice="claude"]').getAttribute("aria-pressed"), "true");
    assert.equal(document.querySelector('[data-provider-enabled="claude"]').disabled, false);
    assert.match(document.querySelector("#save-status").textContent, /Could not write config/);
  } finally { harness.close(); }
});

test("disabled active providers show disabled status and Save and use explicitly enables them", async () => {
  const initial = config({ provider: { kind: "claude_cli", auth: "api_key", model: "sonnet", enabled: false } });
  const harness = makeHarness({ config: initial });
  try {
    await harness.ready();
    const { document } = harness.dom.window;
    assert.equal(document.querySelector('[data-provider-enabled="claude"]').getAttribute("aria-checked"), "false");
    assert.match(document.querySelector('[data-provider-choice="claude"]').textContent, /Active · Disabled/);
    assert.match(document.querySelector("#status-provider").textContent, /Disabled/);
    assert.ok(document.querySelector("#status-check-provider").disabled);
    document.querySelector("#save-models").click();
    await harness.idle(6);
    const request = callsFor(harness, "save_config_section")[0].args;
    assert.equal(request.section, "provider");
    assert.equal(request.value.enabled, true);
  } finally { harness.close(); }
});

test("restores saved connection fields when switching between ChatGPT and API modes", async () => {
  const connected = { state: "connected", logged_in: true, via_chatgpt: true, email: "user@example.test", plan: "pro", message: "ChatGPT account connected" };
  const chatgpt = { kind: "open_ai_compatible", auth: "chatgpt", model: "chat-saved", base_url: "https://chat.example/v1", api_key: null, codex_home: "/codex-chat", enabled: true };
  const api = { kind: "open_ai_compatible", auth: "api_key", model: "api-saved", base_url: "https://saved-provider.example/v1", api_key: "fixture-key", codex_home: "/codex-api", enabled: true };
  const persisted = config({ provider: chatgpt, provider_connections: { "open_ai_compatible:chatgpt": chatgpt, "open_ai_compatible:api_key": api } });
  const savedApi = config({ provider: api, provider_connections: { "open_ai_compatible:chatgpt": chatgpt, "open_ai_compatible:api_key": api } });
  const harness = makeHarness({ config: persisted, auth: connected, saveQueue: [savedApi] });
  try {
    await harness.ready();
    const { document } = harness.dom.window;
    assert.equal(document.querySelector("#provider-auth").value, "chatgpt");
    assert.equal(document.querySelector("#model-select").value, "chat-saved");
    assert.equal(document.querySelector("#codex_home").value, "/codex-chat");

    document.querySelector('[data-provider-choice="openai"]').click();
    await harness.idle(2);
    assert.equal(document.querySelector("#provider-auth").value, "api_key");
    assert.equal(document.querySelector("#model").value, "api-saved");
    assert.equal(document.querySelector("#base_url").value, "https://saved-provider.example/v1");
    assert.equal(document.querySelector("#api_key").value, "fixture-key");
    assert.equal(document.querySelector("#codex_home").value, "/codex-api");

    const keychain = document.querySelector("#store-keychain");
    keychain.checked = false;
    keychain.dispatchEvent(new harness.dom.window.Event("change", { bubbles: true }));
    document.querySelector("#save-models").click();
    await harness.idle(8);
    const request = callsFor(harness, "save_config_section").at(-1).args.value;
    assert.equal(request.kind, "open_ai_compatible");
    assert.equal(request.auth, "api_key");
    assert.equal(request.model, "api-saved");
    assert.equal(request.base_url, "https://saved-provider.example/v1");
    assert.equal(request.api_key, "fixture-key");
    assert.equal(request.codex_home, "/codex-api");

    document.querySelector('[data-provider-choice="codex"]').click();
    await harness.idle(2);
    assert.equal(document.querySelector("#provider-auth").value, "chatgpt");
    assert.equal(document.querySelector("#model-select").value, "chat-saved");
    assert.equal(document.querySelector("#codex_home").value, "/codex-chat");
    assert.match(document.querySelector("#chatgpt-status-line").textContent, /Connected/);
  } finally { harness.close(); }
});

test("keeps independent unsaved fields and keychain choices for provider connections", async () => {
  const connected = { state: "connected", logged_in: true, via_chatgpt: true, email: "user@example.test", plan: "pro", message: "ChatGPT account connected" };
  const chatgpt = { kind: "open_ai_compatible", auth: "chatgpt", model: "chat-saved", base_url: "https://chat.example/v1", api_key: null, codex_home: "/codex-chat", enabled: true };
  const api = { kind: "open_ai_compatible", auth: "api_key", model: "api-saved", base_url: "https://saved-provider.example/v1", api_key: "fixture-key", codex_home: "/codex-api", enabled: true };
  const anthropic = { kind: "anthropic", auth: "api_key", model: "anthropic-saved", base_url: "https://api.anthropic.com", api_key: "anthropic-key", codex_home: "/codex-anthropic", enabled: true };
  const harness = makeHarness({
    config: config({ provider: chatgpt, provider_connections: {
      "open_ai_compatible:chatgpt": chatgpt,
      "open_ai_compatible:api_key": api,
      "anthropic:api_key": anthropic,
    } }),
    auth: connected,
  });
  try {
    await harness.ready();
    const { document } = harness.dom.window;
    document.querySelector('[data-provider-choice="openai"]').click();
    await harness.idle(2);
    value(harness.dom, "model", "api-draft");
    value(harness.dom, "base_url", "https://api-draft.example/v1");
    value(harness.dom, "api_key", "api-draft-key");
    value(harness.dom, "codex_home", "/codex-api-draft");
    const apiKeychain = document.querySelector("#store-keychain");
    apiKeychain.checked = false;
    apiKeychain.dispatchEvent(new harness.dom.window.Event("change", { bubbles: true }));

    document.querySelector('[data-provider-choice="codex"]').click();
    await harness.idle(2);
    assert.equal(document.querySelector("#model-select").value, "chat-saved");
    assert.equal(document.querySelector("#codex_home").value, "/codex-chat");
    const chatOption = document.createElement("option");
    chatOption.value = "chat-draft";
    chatOption.textContent = "chat-draft";
    document.querySelector("#model-select").append(chatOption);
    value(harness.dom, "model-select", "chat-draft");
    value(harness.dom, "codex_home", "/codex-chat-draft");

    document.querySelector('[data-provider-choice="anthropic"]').click();
    await harness.idle(2);
    assert.equal(document.querySelector("#model").value, "anthropic-saved");
    assert.equal(document.querySelector("#base_url").value, "https://api.anthropic.com");
    value(harness.dom, "model", "anthropic-draft");
    value(harness.dom, "api_key", "anthropic-draft-key");

    document.querySelector('[data-provider-choice="openai"]').click();
    await harness.idle(2);
    assert.equal(document.querySelector("#provider-auth").value, "api_key");
    assert.equal(document.querySelector("#model").value, "api-draft");
    assert.equal(document.querySelector("#base_url").value, "https://api-draft.example/v1");
    assert.equal(document.querySelector("#api_key").value, "api-draft-key");
    assert.equal(document.querySelector("#codex_home").value, "/codex-api-draft");
    assert.equal(document.querySelector("#store-keychain").checked, false);

    document.querySelector('[data-provider-choice="codex"]').click();
    await harness.idle(2);
    assert.equal(document.querySelector("#provider-auth").value, "chatgpt");
    assert.equal(document.querySelector("#model-select").value, "chat-draft");
    assert.equal(document.querySelector("#codex_home").value, "/codex-chat-draft");
    document.querySelector('[data-provider-choice="anthropic"]').click();
    await harness.idle(2);
    assert.equal(document.querySelector("#model").value, "anthropic-draft");
    assert.equal(document.querySelector("#api_key").value, "anthropic-draft-key");
    assert.match(document.querySelector("#chatgpt-status-line").textContent, /Connected/);
  } finally { harness.close(); }
});

test("does not clear a different auth draft after a pending keychain write", async () => {
  const store = deferred();
  const save = deferred();
  const connected = { state: "connected", logged_in: true, via_chatgpt: true, email: "user@example.test", plan: "pro", message: "ChatGPT account connected" };
  const chatgpt = { kind: "open_ai_compatible", auth: "chatgpt", model: "chat-saved", base_url: "https://chat.example/v1", api_key: null, codex_home: "/codex-chat", enabled: true };
  const api = { kind: "open_ai_compatible", auth: "api_key", model: "api-saved", base_url: "https://saved-provider.example/v1", api_key: "api-key", codex_home: "/codex-api", enabled: true };
  const persisted = config({ provider: api, provider_connections: { "open_ai_compatible:chatgpt": chatgpt, "open_ai_compatible:api_key": api } });
  const harness = makeHarness({ config: persisted, auth: connected, storeQueue: [store], saveQueue: [save] });
  try {
    await harness.ready();
    const { document } = harness.dom.window;
    document.querySelector("#save-models").click();
    await harness.idle(2);
    assert.equal(callsFor(harness, "store_api_key").length, 1);

    document.querySelector('[data-provider-choice="codex"]').click();
    await harness.idle(2);
    value(harness.dom, "api_key", "chat-draft-secret");
    store.resolve("keychain");
    await harness.idle(3);
    assert.equal(document.querySelector("#api_key").value, "chat-draft-secret");

    save.resolve(config({ provider: { ...api, api_key: null }, provider_connections: { "open_ai_compatible:chatgpt": chatgpt, "open_ai_compatible:api_key": { ...api, api_key: null } } }));
    await harness.idle(8);
    document.querySelector('[data-provider-choice="openai"]').click();
    await harness.idle(2);
    assert.equal(document.querySelector("#api_key").value, "", "the submitted API key is cleared only on the API connection");
    document.querySelector('[data-provider-choice="codex"]').click();
    await harness.idle(2);
    assert.equal(document.querySelector("#api_key").value, "chat-draft-secret", "the ChatGPT draft key is retained independently");
  } finally { harness.close(); }
});


test("history preserves incomplete rewrites with accurate recovery guidance", async () => {
  const harness = makeHarness({ history: [
    { ts: 1, label: "Rewrite", kind: "replace", original: "Original", result: "Saved rewrite", outcome: "not_applied" },
    { ts: 2, label: "Rewrite", kind: "replace", original: "Original", result: "Unverified rewrite", outcome: "paste_unverified" },
  ] });
  try {
    const { document } = harness.dom.window;
    document.querySelector('[data-section="history"]').click();
    await harness.idle(2);
    const rows = document.querySelectorAll(".hist-item");
    assert.match(rows[0].textContent, /Not applied/);
    assert.match(rows[0].textContent, /Your selection was not changed/);
    assert.match(rows[0].textContent, /Saved rewrite/);
    assert.match(rows[1].textContent, /Paste unverified/);
    assert.match(rows[1].textContent, /paste may have completed/);
    assert.equal(rows[1].querySelector('[data-act="copy-result"]').disabled, false);
  } finally { harness.close(); }
});

function pressShortcut(harness, init, type = "keydown") {
  const event = new harness.dom.window.KeyboardEvent(type, { bubbles: true, cancelable: true, ...init });
  harness.dom.window.document.querySelector("#cmd-hotkey").dispatchEvent(event);
  return event;
}

async function openShortcutRecorder(harness, id = "rewrite") {
  await harness.ready();
  const document = harness.dom.window.document;
  document.querySelector('[data-section="commands"]').click();
  document.querySelector(`[data-id="${id}"]`).click();
  const button = document.querySelector("#cmd-hotkey");
  button.click();
  await harness.idle();
  assert.equal(document.activeElement, button, "pointer activation must focus the recorder in WebKit");
  return button;
}

const shortcutCommand = { id: "rewrite", label: "Rewrite", prompt: "Rewrite this.", kind: "replace", hotkey: "cmd+shift+r", apps: [] };

test("records a pressed command shortcut, waits for release, and saves its canonical chord", async () => {
  const harness = makeHarness({ config: config({ commands: [shortcutCommand] }) });
  try {
    const button = await openShortcutRecorder(harness);
    assert.equal(button.textContent, "Press shortcut…");
    assert.equal(callsFor(harness, "set_shortcut_recording")[0].args.active, true);
    const chord = { key: "P", code: "KeyP", metaKey: true, shiftKey: true };
    assert.equal(pressShortcut(harness, chord).defaultPrevented, true);
    assert.equal(button.value, "shift+cmd+p");
    assert.equal(button.textContent, "⇧⌘P");
    assert.equal(callsFor(harness, "save_config_section").length, 0);
    assert.equal(callsFor(harness, "set_shortcut_recording").length, 1, "held keys keep global bindings paused");
    pressShortcut(harness, chord, "keyup");
    await harness.idle();
    assert.equal(callsFor(harness, "set_shortcut_recording").at(-1).args.active, false);
    assert.equal(button.getAttribute("aria-pressed"), "false");
    harness.dom.window.document.querySelector("#cmd-save").click();
    await harness.idle();
    assert.equal(callsFor(harness, "save_config_section")[0].args.value[0].hotkey, "shift+cmd+p");
  } finally { harness.close(); }
});

test("recording rejects duplicate and reserved chords without replacing the draft shortcut", async () => {
  const harness = makeHarness({ config: config({ commands: [shortcutCommand, { ...shortcutCommand, id: "other", label: "Other", hotkey: " Command + Option + K " }] }) });
  try {
    const button = await openShortcutRecorder(harness);
    const hint = () => harness.dom.window.document.querySelector("#cmd-hotkey-hint").textContent;
    pressShortcut(harness, { key: "k", code: "KeyK", metaKey: true, altKey: true });
    assert.match(hint(), /already assigned to “Other”/);
    assert.equal(button.value, shortcutCommand.hotkey);
    pressShortcut(harness, { key: " ", code: "Space", ctrlKey: true, shiftKey: true });
    assert.match(hint(), /Custom instruction/);
    pressShortcut(harness, { key: "z", code: "KeyZ", metaKey: true });
    assert.match(hint(), /reserved/);
    pressShortcut(harness, { key: "R", code: "KeyR", metaKey: true, shiftKey: true });
    assert.equal(button.value, "shift+cmd+r", "the command can retain its own shortcut");
  } finally { harness.close(); }
});

test("shortcut capture handles physical Option keys and ignores modifiers, repeats, and unsupported keys", async () => {
  const harness = makeHarness({ config: config({ commands: [shortcutCommand] }) });
  try {
    const button = await openShortcutRecorder(harness);
    for (const event of [
      { key: "Meta", code: "MetaLeft", metaKey: true },
      { key: "a", code: "KeyA" },
      { key: "AudioVolumeUp", code: "AudioVolumeUp", metaKey: true },
      { key: "p", code: "KeyP", metaKey: true, repeat: true },
      { key: "Process", code: "KeyP", metaKey: true, isComposing: true },
    ]) pressShortcut(harness, event);
    assert.equal(button.value, shortcutCommand.hotkey);
    pressShortcut(harness, { key: "π", code: "KeyP", altKey: true });
    assert.equal(button.value, "alt+p", "Option-generated characters must use the physical key code");
  } finally { harness.close(); }
});

test("Escape cancels recording without closing the sheet and Cmd-Enter records without saving", async () => {
  const harness = makeHarness({ config: config({ commands: [shortcutCommand] }) });
  try {
    const button = await openShortcutRecorder(harness);
    pressShortcut(harness, { key: "Enter", code: "Enter", metaKey: true });
    assert.equal(button.value, "cmd+enter");
    assert.equal(callsFor(harness, "save_config_section").length, 0);
    pressShortcut(harness, { key: "Escape", code: "Escape" });
    await harness.idle();
    assert.equal(button.value, shortcutCommand.hotkey);
    assert.ok(harness.dom.window.document.querySelector("#sheet-root"));
    assert.equal(callsFor(harness, "set_shortcut_recording").at(-1).args.active, false);
  } finally { harness.close(); }
});

test("Delete clears a recorded shortcut and Clear saves no shortcut", async () => {
  const harness = makeHarness({ config: config({ commands: [shortcutCommand] }) });
  try {
    const button = await openShortcutRecorder(harness);
    pressShortcut(harness, { key: "Backspace", code: "Backspace" });
    pressShortcut(harness, { key: "Backspace", code: "Backspace" }, "keyup");
    await harness.idle();
    assert.equal(button.value, "");
    assert.equal(button.textContent, "Record shortcut");
    assert.match(harness.dom.window.document.querySelector("#cmd-hotkey-hint").textContent, /cleared/);
    harness.dom.window.document.querySelector("#cmd-save").click();
    await harness.idle();
    assert.equal(callsFor(harness, "save_config_section")[0].args.value[0].hotkey, null);
  } finally { harness.close(); }
});

test("Tab, focus loss, and closing the sheet release shortcut recording", async () => {
  for (const end of ["tab", "blur", "close"]) {
    const harness = makeHarness({ config: config({ commands: [shortcutCommand] }) });
    try {
      const button = await openShortcutRecorder(harness);
      const document = harness.dom.window.document;
      if (end === "tab") pressShortcut(harness, { key: "Tab", code: "Tab" });
      if (end === "blur") document.querySelector("#cmd-label").focus();
      if (end === "close") document.querySelector("#cmd-cancel").click();
      await harness.idle();
      assert.equal(callsFor(harness, "set_shortcut_recording").at(-1).args.active, false, end);
      if (end !== "close") assert.equal(button.getAttribute("aria-pressed"), "false");
    } finally { harness.close(); }
  }
});

test("a late recorder start acknowledgement cannot reactivate a closed sheet", async () => {
  const pending = deferred();
  const harness = makeHarness({ config: config({ commands: [shortcutCommand] }), recordShortcut: ({ active }) => active ? pending.promise : Promise.resolve() });
  try {
    await openShortcutRecorder(harness);
    const document = harness.dom.window.document;
    assert.equal(document.querySelector("#cmd-hotkey").textContent, "Preparing…");
    document.querySelector("#cmd-cancel").click();
    pending.resolve();
    await harness.idle(5);
    assert.equal(document.querySelector("#sheet-root"), null);
    assert.deepEqual(callsFor(harness, "set_shortcut_recording").map(({ args }) => args.active), [true, false]);
  } finally { harness.close(); }
});

test("failed recorder preparation keeps the existing shortcut and explains the error", async () => {
  const harness = makeHarness({ config: config({ commands: [shortcutCommand] }), recordShortcut: ({ active }) => active ? Promise.reject(new Error("Finish the active command first")) : Promise.resolve() });
  try {
    const button = await openShortcutRecorder(harness);
    assert.equal(button.value, shortcutCommand.hotkey);
    assert.equal(button.getAttribute("aria-pressed"), "false");
    assert.match(harness.dom.window.document.querySelector("#cmd-hotkey-hint").textContent, /Finish the active command first/);
  } finally { harness.close(); }
});

test("releasing Command finishes capture even when WebKit omits the letter keyup", async () => {
  const harness = makeHarness({ config: config({ commands: [shortcutCommand] }) });
  try {
    const button = await openShortcutRecorder(harness);
    pressShortcut(harness, { key: "k", code: "KeyK", metaKey: true });
    pressShortcut(harness, { key: "Meta", code: "MetaLeft", metaKey: false }, "keyup");
    await harness.idle();
    assert.equal(button.value, "cmd+k");
    assert.equal(button.getAttribute("aria-pressed"), "false");
    assert.equal(callsFor(harness, "set_shortcut_recording").at(-1).args.active, false);
  } finally { harness.close(); }
});
