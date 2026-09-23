// Dev-only stand-in for the Tauri bridge so the Settings UI runs in a plain
// browser at http://localhost:1420. Injected by vite.config.js only when
// `SELARA_MOCK=1` (`npm run dev:mock`); never part of `vite build` or dist/.
//
// URL parameters:
//   ?scenario=configured|fresh|update|errors   starting state (default configured)
//   &fail=config                                make get_config reject too
//   &delay=<ms>                                 per-call latency (default 150)
//   &accent=<hex>                               system accent, e.g. %23bf5af2 (default: none)
//
// window.__selaraMock exposes { emit(event, payload), calls, state } for tests
// and for poking at the UI from the console.
(function () {
  "use strict";
  if (window.__TAURI_INTERNALS__ || (window.__TAURI__ && window.__TAURI__.core)) return;

  const params = new URLSearchParams(location.search);
  const scenario = params.get("scenario") || "configured";
  const failConfig = params.get("fail") === "config";
  const delayParam = Number(params.get("delay"));
  const delay = Number.isFinite(delayParam) && delayParam >= 0 ? delayParam : 150;
  const accent = /^#[0-9a-f]{6}$/i.test(params.get("accent") || "") ? params.get("accent") : null;
  const now = Math.floor(Date.now() / 1000);

  const clone = (value) => (value === undefined ? value : JSON.parse(JSON.stringify(value)));
  const connectionKey = (p) => p.kind + ":" + (p.auth || "api_key");

  const CLI_KINDS = ["claude_cli", "cursor_cli", "open_code_cli"];
  const DEFAULT_BASE_URL = {
    open_ai_compatible: "https://api.openai.com/v1",
    open_router: "https://openrouter.ai/api/v1",
    anthropic: "https://api.anthropic.com",
  };

  function command(id, label, prompt, hotkey, extra) {
    return { id, label, kind: "replace", prompt, hotkey: hotkey || null, ...extra };
  }

  const baseCommands = [
    command("proofread", "Proofread", "Fix spelling, grammar, and punctuation. Keep the original meaning and tone.", "ctrl+alt+p"),
    command("rewrite", "Rewrite", "Rewrite the text to read more clearly.", "ctrl+alt+r"),
    command("friendly", "Friendly", "Rewrite the text in a warm, friendly tone."),
    command("professional", "Professional", "Rewrite the text in a clear, professional tone.", "ctrl+alt+shift+p"),
    command("concise", "Concise", "Make the text shorter without losing meaning.", null, { model: "gpt-5.4-mini" }),
    command("translate", "Translate", "Translate the text to English.", null, { apps: ["com.apple.mail", "com.tinyspeck.slackmacgap"] }),
  ];

  const limits = { soft_warn_chars: 8000, hard_max_chars: 100000, replace_warn_chars: 4000, secret_guard: true };

  function makeConfig() {
    if (scenario === "fresh") {
      return {
        schema_version: 2,
        provider: { kind: "open_ai_compatible", enabled: true, base_url: "", model: "", api_key: null, auth: "api_key", codex_home: null, cli_binary: null },
        provider_connections: {},
        hotkey: "ctrl+shift+space",
        language: "en",
        excluded_apps: [],
        commands: [],
        limits: clone(limits),
      };
    }
    const provider = { kind: "open_ai_compatible", enabled: true, base_url: "https://api.openai.com/v1", model: "gpt-5.4-mini", api_key: null, auth: "api_key", codex_home: null, cli_binary: null };
    return {
      schema_version: 2,
      provider,
      provider_connections: {
        "open_ai_compatible:api_key": clone(provider),
        "claude_cli:api_key": { kind: "claude_cli", enabled: true, base_url: "", model: "", api_key: null, auth: "api_key", codex_home: null, cli_binary: null },
      },
      hotkey: "ctrl+shift+space",
      language: "en",
      excluded_apps: ["com.1password.1password"],
      commands: clone(baseCommands),
      limits: clone(limits),
    };
  }

  function bucket(requests, input, output, cost) {
    return { requests, input, output, cost_usd: cost, unpriced: 0, tokens_missing: 0 };
  }

  function makeUsage() {
    if (scenario === "fresh") {
      return { path: "~/Library/Application Support/selara/usage.jsonl", today: bucket(0, 0, 0, 0), last_30_days: bucket(0, 0, 0, 0), all_time: bucket(0, 0, 0, 0), models: [] };
    }
    return {
      path: "~/Library/Application Support/selara/usage.jsonl",
      today: bucket(12, 4180, 3920, 0.0061),
      last_30_days: bucket(284, 102455, 96120, 0.1482),
      all_time: { ...bucket(1031, 391204, 370880, 0.5427), unpriced: 42, tokens_missing: 3 },
      models: [
        { kind: "openai_compatible", model: "gpt-5.4-mini", ...bucket(861, 330100, 311540, 0.5427) },
        { kind: "claude_cli", model: "", ...bucket(128, 48920, 47110, null), unpriced: 128 },
        { kind: "openrouter", model: "anthropic/claude-opus-5", ...bucket(42, 12184, 12230, null), unpriced: 42, tokens_missing: 3 },
      ],
    };
  }

  function makeHistory() {
    if (scenario === "fresh") return [];
    return [
      { ts: now - 90, command_id: "proofread", label: "Proofread", kind: "replace", app: "Mail", original: "Thanks for you're help with the launch, its been great.", result: "Thanks for your help with the launch; it's been great.", outcome: "applied" },
      { ts: now - 3600 * 2, command_id: "concise", label: "Concise", kind: "replace", app: "Slack", original: "I just wanted to quickly follow up and check in on whether you had a chance to take a look at the document I sent over last week.", result: "Did you get a chance to review the document I sent last week?", outcome: "not_applied" },
      { ts: now - 86400, command_id: "professional", label: "Professional", kind: "replace", app: "Notes", original: "hey can u send the numbers asap", result: "Could you send the numbers when you have a moment?", outcome: "paste_unverified" },
      { ts: now - 86400 * 3, command_id: "translate", label: "Translate", kind: "replace", app: "Safari", original: "Merci beaucoup pour votre patience.", result: "Thank you very much for your patience.", outcome: "applied" },
    ];
  }

  const signedOut = { state: "signed_out", logged_in: false, via_chatgpt: false, message: "No ChatGPT account is signed in" };
  const connected = { state: "connected", logged_in: true, via_chatgpt: true, email: "user@example.test", plan: "pro", message: "ChatGPT account connected" };

  const running = scenario === "configured" || scenario === "update";
  const state = {
    scenario,
    config: makeConfig(),
    apiKeySource: scenario === "fresh" ? "none" : "keychain",
    auth: scenario === "configured" ? clone(connected) : clone(signedOut),
    ax: scenario === "fresh" ? "missing" : "granted",
    autostart: scenario !== "fresh",
    serveRunning: running,
    history: makeHistory(),
    usage: makeUsage(),
    update: scenario === "update"
      ? { state: "available", current: "0.6.0", version: "0.7.0", date: "2026-09-22T00:00:00Z", notes: "### Features\n\n* Faster command menu\n* Native shortcut recorder\n\n### Fixes\n\n* Keep drafts after a failed save" }
      : null,
    log: running
      ? ["[serve] selara serve 0.6.0 starting", "[serve] accessibility trust: granted", "[serve] 6 commands registered, 4 shortcuts", "[serve] ready"]
      : [],
  };

  function supervisorStatus() {
    if (!state.serveRunning) return { managed: false, pid: null, external: false, last_error: scenario === "errors" ? "serve exited with status 1" : null, child_status: null };
    return {
      managed: true,
      pid: 48213,
      external: false,
      last_error: null,
      child_status: { version: "0.6.0", id: "serve-48213", readiness: "ready", ax_trust: state.ax, generation: 1, external: false },
    };
  }

  function applySection(section, value) {
    const cfg = state.config;
    switch (section) {
      case "general":
        cfg.hotkey = String(value.hotkey || "").trim() || "ctrl+shift+space";
        cfg.language = String(value.language || "").trim() || "en";
        if (Array.isArray(value.excluded_apps)) cfg.excluded_apps = value.excluded_apps.map((a) => String(a).trim()).filter(Boolean);
        break;
      case "provider": {
        const next = clone(value);
        if (CLI_KINDS.includes(next.kind)) {
          next.api_key = null;
          next.base_url = "";
          next.auth = "api_key";
        }
        cfg.provider_connections = cfg.provider_connections || {};
        cfg.provider_connections[connectionKey(cfg.provider)] = clone(cfg.provider);
        cfg.provider_connections[connectionKey(next)] = clone(next);
        cfg.provider = next;
        break;
      }
      case "provider_enabled": {
        const key = connectionKey(value);
        cfg.provider_connections = cfg.provider_connections || {};
        if (connectionKey(cfg.provider) === key) {
          cfg.provider.enabled = !!value.enabled;
          cfg.provider_connections[key] = clone(cfg.provider);
        } else {
          const existing = cfg.provider_connections[key] || {
            kind: value.kind, auth: value.auth || "api_key", base_url: DEFAULT_BASE_URL[value.kind] || "",
            model: "", api_key: null, codex_home: null, cli_binary: null,
          };
          existing.enabled = !!value.enabled;
          cfg.provider_connections[key] = existing;
        }
        break;
      }
      case "commands":
        cfg.commands = clone(value);
        break;
      case "limits":
        cfg.limits = clone(value);
        break;
      default:
        throw "unknown config section `" + section + "` (expected general, provider, provider_enabled, commands, or limits)";
    }
    return clone(cfg);
  }

  const listeners = new Map();
  function emit(event, payload) {
    for (const cb of listeners.get(event) || []) {
      try { cb({ event, id: 0, payload }); } catch (e) { console.warn("mock listener failed", e); }
    }
  }

  function handle(cmd, args) {
    args = args || {};
    switch (cmd) {
      case "get_config":
        if (failConfig) throw "Could not read ~/.config/selara/config.toml: invalid TOML at line 12";
        return clone(state.config);
      case "save_config_section":
        if (scenario === "errors") throw "Could not write ~/.config/selara/config.toml: Permission denied (os error 13)";
        return applySection(args.section, args.value);
      case "save_config":
        state.config = clone(args.config);
        return null;
      case "config_path": return "~/.config/selara/config.toml";
      case "history_path": return "~/Library/Application Support/selara/history.jsonl";
      case "api_key_source": return state.apiKeySource;
      case "store_api_key": state.apiKeySource = "keychain"; return state.apiKeySource;
      case "clear_api_key": state.apiKeySource = "none"; return state.apiKeySource;
      case "cli_provider_status":
        if (args.kind === "cursor_cli") return { installed: false, version: null, binary: null, message: "cursor-agent was not found on PATH." };
        return { installed: true, version: "2.1.4", binary: "/opt/homebrew/bin/" + ({ claude_cli: "claude", open_code_cli: "opencode" }[args.kind] || "cli"), message: "CLI available. Sign in through the CLI before writing." };
      case "export_commands": return "~/Downloads/selara-commands.json";
      case "import_commands": return { added: 2, replaced: 0, skipped: 1, renamed: [["rewrite", "rewrite-2"]], hotkeys_dropped: 1 };
      case "history_list": return clone(state.history);
      case "history_clear": state.history = []; return null;
      case "chatgpt_auth_status": return clone(state.auth);
      case "chatgpt_login": state.auth = clone(connected); return clone(state.auth);
      case "chatgpt_login_cancel": return null;
      case "chatgpt_logout": state.auth = clone(signedOut); return clone(state.auth);
      case "list_chatgpt_models_cmd": return ["gpt-5.4", "gpt-5.4-mini", "gpt-5.4-codex"];
      case "list_provider_models_cmd":
        if (args.kind === "anthropic") return ["claude-opus-5", "claude-sonnet-5", "claude-haiku-4-5"];
        if (args.kind === "open_router") return ["anthropic/claude-opus-5", "openai/gpt-5.4", "google/gemini-2.5-pro"];
        return ["gpt-5.4", "gpt-5.4-mini", "gpt-4o-mini"];
      case "usage_summary": return clone(state.usage);
      case "clear_usage": state.usage = { ...makeUsage(), today: bucket(0, 0, 0, 0), last_30_days: bucket(0, 0, 0, 0), all_time: bucket(0, 0, 0, 0), models: [] }; return clone(state.usage);
      case "serve_status": return { running: state.serveRunning, pid: state.serveRunning ? 48213 : null, pidfile: "~/Library/Application Support/selara/serve.pid" };
      case "serve_supervisor_status": return supervisorStatus();
      case "serve_log": return state.log.slice();
      case "serve_start":
      case "serve_restart":
        if (scenario === "errors") throw "selara serve exited during startup: Accessibility permission is missing";
        state.serveRunning = true;
        state.log.push("[serve] ready");
        setTimeout(() => emit("serve-changed", null), 0);
        return null;
      case "serve_stop":
        state.serveRunning = false;
        state.log.push("[serve] stopped");
        setTimeout(() => emit("serve-changed", null), 0);
        return null;
      case "serve_quiesce":
      case "serve_resume":
        return null;
      case "accessibility_status": return state.ax;
      case "open_accessibility_settings": return null;
      case "app_version": return "0.6.0";
      case "bundled_codex_version": return "0.153.4";
      case "system_accent_color": return accent;
      case "update_status": return clone(state.update);
      case "check_for_updates":
        if (scenario === "errors") throw "Update check failed: network unreachable";
        state.update = state.update && state.update.state === "available" ? state.update : { state: "up_to_date", current: "0.6.0" };
        return clone(state.update);
      case "install_update": return null;
      case "set_shortcut_recording": return null;
      case "plugin:autostart|is_enabled": return state.autostart;
      case "plugin:autostart|enable": state.autostart = true; return null;
      case "plugin:autostart|disable": state.autostart = false; return null;
      default:
        console.warn("[mock-tauri] unhandled command:", cmd, args);
        return null;
    }
  }

  const calls = [];
  function invoke(cmd, args) {
    calls.push({ cmd, args: clone(args) });
    return new Promise((resolve, reject) => {
      setTimeout(() => {
        try { resolve(handle(cmd, args)); } catch (e) { reject(e); }
      }, delay);
    });
  }

  async function listen(event, callback) {
    if (!listeners.has(event)) listeners.set(event, new Set());
    listeners.get(event).add(callback);
    return () => listeners.get(event).delete(callback);
  }

  window.__TAURI__ = { core: { invoke }, event: { listen } };
  window.__selaraMock = { emit, calls, state };

  // The real window is transparent over NSVisualEffectMaterial::Sidebar. A
  // plain browser would show white behind the glass, so paint a neutral
  // stand-in for that material in mock mode only. The attribute selector
  // outranks the page's own `html, body { background: transparent }`.
  document.documentElement.dataset.mockBridge = scenario;
  const style = document.createElement("style");
  style.id = "mock-tauri-backdrop";
  style.textContent =
    "html[data-mock-bridge]{background:linear-gradient(160deg,#e9e7ef 0%,#dcdde3 55%,#d4d9df 100%) fixed}" +
    "@media (prefers-color-scheme: dark){html[data-mock-bridge]{background:linear-gradient(160deg,#2b2a30 0%,#232429 55%,#1e2126 100%) fixed}}";
  document.head.appendChild(style);
  console.info("[mock-tauri] scenario=" + scenario + (failConfig ? " fail=config" : "") + " delay=" + delay + "ms");
})();
