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
      case "history_list": return Promise.resolve(options.history || []);
      case "plugin:autostart|is_enabled": return Promise.resolve(false);
      case "save_config_section": return next(saveQueue, configResult);
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
