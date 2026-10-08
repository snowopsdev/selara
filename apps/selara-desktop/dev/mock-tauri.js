// Dev-only stand-in for the Tauri bridge so the Settings UI runs in a plain
// browser at http://localhost:1420. Injected by vite.config.js only when
// `SELARA_MOCK=1` (`npm run dev:mock`); never part of `vite build` or dist/.
//
// URL parameters:
//   ?scenario=configured|fresh|update|errors   starting state (default configured)
//   &fail=config                                make get_config reject too
//   &delay=<ms>                                 per-call latency (default 150)
//   &accent=<hex>                               system accent, e.g. %23bf5af2 (default: none)
//   &serve=external                             serve was started in Terminal, not by
//                                               Settings: running but unmanaged, so
//                                               accessibility_status is "unknown"
//
// window.__selaraMock exposes { emit(event, payload), calls, state, menu,
// chooseMenuItem(text) } for tests and for poking at the UI from the console.
// Native menus are not drawn; `menu` holds the last one the UI opened.
// Set state.confirm = false to answer native confirmations with Cancel, and
// state.failSaves = true to make save_config_section fail until reset.
// try_command returns a deterministic rewrite chosen by the prompt's wording,
// records usage like the real command, and uses the backend's error strings.
// state.chooseApp answers choose_app (null = Cancel); state.pngCopies and
// state.pngSaves record copy_png / save_png, and state.saveCancelled = true
// makes the save panel return null.
(function () {
  "use strict";
  if (window.__TAURI_INTERNALS__ || (window.__TAURI__ && window.__TAURI__.core)) return;

  const params = new URLSearchParams(location.search);
  const scenario = params.get("scenario") || "configured";
  const failConfig = params.get("fail") === "config";
  const delayParam = params.has("delay") ? Number(params.get("delay")) : NaN;
  const delay = Number.isFinite(delayParam) && delayParam >= 0 ? delayParam : 150;
  const accent = /^#[0-9a-f]{6}$/i.test(params.get("accent") || "") ? params.get("accent") : null;
  const externalServe = params.get("serve") === "external";
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
    // glyph/color/review are optional; Friendly leaves them unset so the UI
    // shows its derived monogram and color.
    command("proofread", "Proofread", "Fix spelling, grammar, and punctuation. Keep the original meaning and tone.", "ctrl+alt+p", { glyph: "✓", color: "#34c759" }),
    command("rewrite", "Rewrite", "Rewrite the text to read more clearly.", "ctrl+alt+r", { glyph: "↻", color: "#0a84ff" }),
    command("friendly", "Friendly", "Rewrite the text in a warm, friendly tone."),
    command("professional", "Professional", "Rewrite the text in a clear, professional tone suited to {{app}}. Reply in {{language}}.", "ctrl+alt+shift+p", { glyph: "◆", color: "#5e5ce6", review: true }),
    command("concise", "Concise", "Make the text shorter without losing meaning.", null, { model: "gpt-5.4-mini", glyph: "✂︎", color: "#ff375f" }),
    command("translate", "Translate", "Translate the text to English.", null, { apps: ["com.apple.mail", "com.tinyspeck.slackmacgap"], glyph: "文", color: "#30b0c7", review: true }),
  ];

  // The nine commands a new config.toml starts with (selara-core builtin_commands).
  const builtinCommands = [
    command("proofread", "Proofread", "Proofread the text. Fix grammar, spelling, and punctuation only. Keep meaning and voice. Return only the corrected text."),
    command("rewrite", "Rewrite", "Rewrite the text for clarity and flow. Keep the original meaning. Return only the rewritten text."),
    command("friendly", "Friendly", "Rewrite the text in a warm, friendly tone. Return only the rewritten text."),
    command("professional", "Professional", "Rewrite the text in a clear, professional tone. Return only the rewritten text."),
    command("concise", "Concise", "Make the text more concise without losing key meaning. Return only the rewritten text."),
    command("summary", "Summary", "Summarize the text clearly in markdown. Use short paragraphs or bullets as needed."),
    command("key_points", "Key Points", "Extract the key points as a markdown bullet list."),
    command("table", "Table", "Convert the useful information in the text into a markdown table."),
    command("translate", "Translate", "Translate the text to {{language}}. Return only the translation."),
  ];

  const limits = { soft_warn_chars: 8000, hard_max_chars: 100000, replace_warn_chars: 4000, secret_guard: true };

  function makeConfig() {
    if (scenario === "fresh") {
      // Matches AppConfig::default(), which load_or_init writes on first launch.
      return {
        schema_version: 2,
        provider: { kind: "open_ai_compatible", enabled: true, base_url: "https://api.openai.com/v1", model: "gpt-4o-mini", api_key: null, auth: "api_key", codex_home: null, cli_binary: null },
        provider_connections: {},
        hotkey: "ctrl+shift+space",
        language: "en",
        excluded_apps: [],
        commands: clone(builtinCommands),
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

  // The 30 local days ending today, oldest first, as usage_summary reports them.
  function localDay(offset) {
    const d = new Date();
    d.setHours(12, 0, 0, 0);
    d.setDate(d.getDate() - offset);
    return d;
  }
  function dayKey(d) {
    return d.getFullYear() + "-" + String(d.getMonth() + 1).padStart(2, "0") + "-" + String(d.getDate()).padStart(2, "0");
  }
  function emptyDaily() {
    return Array.from({ length: 30 }, (_, i) => ({ day: dayKey(localDay(29 - i)), requests: 0, cost_usd: null }));
  }
  // Deterministic and plausible: weekends lighter, `today` today, `total` in all.
  function sampleDaily(total, today, cost) {
    const days = Array.from({ length: 30 }, (_, i) => localDay(29 - i));
    const weekend = (d) => d.getDay() === 0 || d.getDay() === 6;
    let seed = 7;
    const weights = days.slice(0, 29).map((d) => {
      seed = (seed * 9301 + 49297) % 233280;
      return (0.45 + seed / 233280) * (weekend(d) ? 0.3 : 1);
    });
    const sum = weights.reduce((a, b) => a + b, 0);
    const counts = weights.map((w) => Math.max(1, Math.floor((w / sum) * (total - today))));
    let short = total - today - counts.reduce((a, b) => a + b, 0);
    for (let i = 0; short > 0; i = (i + 1) % counts.length) {
      if (!weekend(days[i])) { counts[i] += 1; short -= 1; }
    }
    counts.push(today);
    return days.map((d, i) => ({ day: dayKey(d), requests: counts[i], cost_usd: counts[i] ? Math.round(counts[i] * (cost / total) * 10000) / 10000 : null }));
  }

  function emptyUsage() {
    const empty = () => bucket(0, 0, 0, null);
    return { path: "~/Library/Application Support/selara/usage.jsonl", today: empty(), last_30_days: empty(), all_time: empty(), models: [], daily: emptyDaily(), providers: [] };
  }

  function makeUsage() {
    if (scenario === "fresh") {
      return emptyUsage();
    }
    return {
      path: "~/Library/Application Support/selara/usage.jsonl",
      today: bucket(12, 4180, 3920, 0.0061),
      last_30_days: bucket(284, 102455, 96120, 0.1482),
      all_time: { ...bucket(1031, 391204, 370880, 0.5427), unpriced: 170, tokens_missing: 3 },
      models: [
        { kind: "openai_compatible", model: "gpt-5.4-mini", ...bucket(861, 330100, 311540, 0.5427) },
        { kind: "claude_cli", model: "", ...bucket(128, 48920, 47110, null), unpriced: 128 },
        { kind: "openrouter", model: "anthropic/claude-opus-5", ...bucket(42, 12184, 12230, null), unpriced: 42, tokens_missing: 3 },
      ],
      daily: sampleDaily(284, 12, 0.1482),
      // Timed requests (duration_ms) exist for the API key and Claude Code;
      // the OpenRouter runs predate timing, so it has none.
      providers: [
        { kind: "openai_compatible", last_ts: now - 120, median_ms: 1100, recent_ms: [980, 1240, 870, 1530, 1090, 1180, 760, 1420, 1010, 940, 1300, 1110, 890, 1650, 1020, 1150, 990, 1270, 1080, 1100], last_30_days: bucket(236, 85010, 79800, 0.1482) },
        { kind: "claude_cli", last_ts: now - 3600 * 3 - 600, median_ms: 4200, recent_ms: [3900, 4600, 3800, 5200, 4100, 4300, 3700, 4900, 4200, 4000, 4500, 4150], last_30_days: { ...bucket(38, 13880, 12900, null), unpriced: 38 } },
        { kind: "openrouter", last_ts: now - 86400 * 6, median_ms: null, recent_ms: [], last_30_days: { ...bucket(10, 3565, 3420, null), unpriced: 10 } },
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

  // Per-provider activity (UsageSummary.providers): one entry per usage kind
  // label seen. Added only when the fixture doesn't carry its own.
  function withProviderActivity(usage) {
    if (Array.isArray(usage.providers)) return usage;
    const recent = (base) => Array.from({ length: 20 }, (_, i) => base + ((i * 137) % 900) - 300);
    const providers = scenario === "fresh" ? [] : [
      { kind: "openai_compatible", last_ts: now - 90, median_ms: 1140, recent_ms: recent(1140), last_30_days: bucket(231, 84120, 79010, 0.1482) },
      { kind: "claude_cli", last_ts: now - 86400 * 2, median_ms: 3820, recent_ms: recent(3820), last_30_days: { ...bucket(41, 15900, 14880, null), unpriced: 41 } },
      { kind: "openrouter", last_ts: now - 86400 * 9, median_ms: null, recent_ms: [], last_30_days: { ...bucket(12, 2435, 2230, null), unpriced: 12, tokens_missing: 3 } },
    ];
    return { ...usage, providers };
  }

  // try_command: a deterministic stand-in for a model run. The rewrite
  // follows the prompt's wording; errors use the backend's exact strings.
  const KNOWN_REWRITES = {
    "hey can u send the numbers asap": {
      professional: "Could you send the numbers when you have a moment?",
      friendly: "Hey! Could you send over the numbers when you get a chance? Thanks so much!",
      concise: "Please send the numbers ASAP.",
      proofread: "Hey, can you send the numbers ASAP?",
    },
    "Thanks for you're help with the launch, its been great.": {
      professional: "Thank you for your help with the launch; it has been a great success.",
      friendly: "Thanks so much for your help with the launch, it's been great!",
      concise: "Thanks for your help with the launch.",
      proofread: "Thanks for your help with the launch; it's been great.",
    },
  };
  function tidy(text) {
    let t = " " + text.trim() + " ";
    for (const [a, b] of [[" u ", " you "], [" ur ", " your "], [" you're help", " your help"], [", its ", "; it's "], [" its ", " it's "], [" asap", " ASAP"], [" i ", " I "], [" dont ", " don't "], [" cant ", " can't "]]) t = t.split(a).join(b);
    t = t.trim();
    t = t.charAt(0).toUpperCase() + t.slice(1);
    return /[.!?]$/.test(t) ? t : t + ".";
  }
  function fakeRewrite(prompt, text) {
    const p = prompt.toLowerCase();
    const style = /translat/.test(p) ? "translate" : /professional|formal/.test(p) ? "professional" : /friendly|warm/.test(p) ? "friendly"
      : /concise|shorter|shorten/.test(p) ? "concise" : /summar|key points|bullet|table/.test(p) ? "summary" : /proofread|grammar|spelling/.test(p) ? "proofread" : "rewrite";
    const known = KNOWN_REWRITES[text.trim()];
    if (known && known[style]) return known[style];
    if (style === "translate") return { "Merci beaucoup pour votre patience.": "Thank you very much for your patience." }[text.trim()] || tidy(text);
    if (style === "summary") return "- " + tidy(text.split(/(?<=[.!?])\s/)[0]);
    if (style === "concise") {
      const words = tidy(text).replace(/\b(just|really|quickly|very|actually|basically) /gi, "").split(" ");
      return words.slice(0, Math.max(4, Math.ceil(words.length * 0.6))).join(" ").replace(/[,;:.]?$/, ".");
    }
    if (style === "friendly") return "Hi! " + tidy(text).replace(/\.$/, "!");
    if (style === "professional") return tidy(text).replace(/^Hey,? /, "Hello, ").replace(/^Thanks\b/, "Thank you");
    return tidy(text);
  }
  function usageLabel(p) {
    if (p.kind === "open_ai_compatible") return p.auth === "chatgpt" ? "chatgpt_codex" : "openai_compatible";
    return p.kind === "open_router" ? "openrouter" : p.kind;
  }
  function tryCommand(args) {
    const prompt = String(args.prompt || "");
    const text = String(args.text || "");
    if (!prompt.trim()) throw "Write an instruction for the command first.";
    if (!text.trim()) throw "Enter some sample text to try the command on.";
    const max = Number(state.config.limits && state.config.limits.hard_max_chars) || 0;
    if (max && text.length > max) throw "Sample is " + text.length + " characters, over your hard limit of " + max + ". Shorten it or change Limits in Settings.";
    const p = state.config.provider;
    const kind = usageLabel(p);
    // Otherwise the provider's own message, as the real command reports it.
    if (kind === "openai_compatible" && !state.keychain.includes(p.kind) && !p.api_key) throw "OpenAI-compatible request failed: 401 Unauthorized (no API key configured)";
    if (kind === "chatgpt_codex" && !state.auth.logged_in) throw "Codex is not signed in. Sign in with ChatGPT in Providers.";
    const model = String(args.model || p.model || "").trim() || (CLI_KINDS.includes(p.kind) ? "default" : "gpt-4o-mini");
    const elapsed = 600 + ((text.length * 37 + prompt.length * 11) % 1400);
    const result = fakeRewrite(prompt, text);
    // A run records usage, like the real command.
    const input = Math.ceil(text.length / 4) + Math.ceil(prompt.length / 4);
    const output = Math.ceil(result.length / 4);
    for (const key of ["today", "last_30_days", "all_time"]) {
      const b = state.usage[key];
      b.requests += 1; b.input += input; b.output += output;
    }
    state.usage.providers = state.usage.providers || [];
    let entry = state.usage.providers.find((e) => e.kind === kind);
    if (!entry) { entry = { kind, last_ts: 0, median_ms: null, recent_ms: [], last_30_days: bucket(0, 0, 0, null) }; state.usage.providers.push(entry); }
    entry.last_ts = Math.floor(Date.now() / 1000);
    entry.recent_ms = [...entry.recent_ms, elapsed].slice(-20);
    const sorted = [...entry.recent_ms].sort((a, b) => a - b);
    entry.median_ms = sorted[Math.floor(sorted.length / 2)];
    entry.last_30_days.requests += 1;
    return { result, elapsed_ms: elapsed, model, provider: kind };
  }

  const signedOut = { state: "signed_out", logged_in: false, via_chatgpt: false, message: "No ChatGPT account is signed in" };
  const connected = { state: "connected", logged_in: true, via_chatgpt: true, email: "user@example.test", plan: "pro", message: "ChatGPT account connected" };

  const running = externalServe || scenario === "configured" || scenario === "update";
  const state = {
    scenario,
    config: makeConfig(),
    // Provider kinds with a key in the (mock) keychain, like secrets::keychain_*.
    keychain: scenario === "fresh" ? [] : ["open_ai_compatible"],
    auth: scenario === "configured" ? clone(connected) : clone(signedOut),
    ax: scenario === "fresh" ? "missing" : "granted",
    autostart: scenario !== "fresh",
    confirm: true,
    // choose_app answers with this app; null acts as Cancel.
    chooseApp: { name: "Keychain Access", bundle_id: "com.apple.keychainaccess" },
    // copy_png / save_png record what reached the pasteboard and the save panel.
    pngCopies: [],
    pngSaves: [],
    saveCancelled: false,
    serveRunning: running,
    // A serve started outside Settings: the app can't inspect its grant.
    serveExternal: externalServe,
    history: makeHistory(),
    usage: withProviderActivity(makeUsage()),
    update: scenario === "update"
      ? { state: "available", current: "0.7.0", version: "0.8.0", date: "2026-10-22T00:00:00Z", notes: "### Features\n\n* Faster command menu\n* Native shortcut recorder\n\n### Fixes\n\n* Keep drafts after a failed save" }
      : null,
    log: running
      ? ["[serve] selara serve 0.7.0 starting", "[serve] accessibility trust: granted", "[serve] 6 commands registered, 4 shortcuts", "[serve] ready"]
      : [],
  };

  // Regular (Dock) apps, sorted by name, without Selara: running_apps.
  const RUNNING_APPS = [
    { name: "1Password", bundle_id: "com.1password.1password" },
    { name: "Finder", bundle_id: "com.apple.finder" },
    { name: "Mail", bundle_id: "com.apple.mail" },
    { name: "Messages", bundle_id: "com.apple.MobileSMS" },
    { name: "Notes", bundle_id: "com.apple.Notes" },
    { name: "Safari", bundle_id: "com.apple.Safari" },
    { name: "Slack", bundle_id: "com.tinyspeck.slackmacgap" },
    { name: "Terminal", bundle_id: "com.apple.Terminal" },
    { name: "Visual Studio Code", bundle_id: "com.microsoft.VSCode" },
  ];
  const ICON_COLORS = { "1password": "#0572ec", finder: "#1e90ff", mail: "#2f8cff", messages: "#34c759", notes: "#e9b500", safari: "#0a84ff", slack: "#4a154b", terminal: "#1d1d1f", "visual studio code": "#1f8ad2", "keychain access": "#8e8e93" };
  // app_icon: a 64×64 image for a name or bundle id; null for globs and
  // unknown apps, where NSWorkspace has nothing to show.
  function appIcon(app) {
    const raw = String(app || "").trim();
    if (!raw || raw.includes("*")) return null;
    const known = [...RUNNING_APPS, { name: "Keychain Access", bundle_id: "com.apple.keychainaccess" }]
      .find((a) => a.bundle_id.toLowerCase() === raw.toLowerCase() || a.name.toLowerCase() === raw.toLowerCase());
    if (!known) return null;
    const letters = /^\d/.test(known.name) ? known.name.slice(0, 2).toUpperCase() : known.name[0];
    const svg = "<svg xmlns='http://www.w3.org/2000/svg' width='64' height='64'><rect x='4' y='4' width='56' height='56' rx='13' fill='" + (ICON_COLORS[known.name.toLowerCase()] || "#8e8e93") + "'/>" +
      "<text x='32' y='42' font-family='-apple-system,Helvetica,sans-serif' font-size='26' font-weight='700' fill='white' text-anchor='middle'>" + letters + "</text></svg>";
    return "data:image/svg+xml;utf8," + encodeURIComponent(svg);
  }

  // The native command reports the active provider's source only.
  function keySource() {
    const kind = state.config.provider.kind;
    if (CLI_KINDS.includes(kind)) return "none";
    return state.keychain.includes(kind) ? "keychain" : "none";
  }

  // A fixed two-command pack merged the way commands::merge_commands does:
  // "rewrite" collides by id, and "summary" asks for Proofread's shortcut.
  function importPack(mode) {
    const incoming = [
      command("rewrite", "Rewrite", "Rewrite the text so it reads naturally. Return only the rewritten text."),
      command("summary", "Summary", "Summarize the text in three sentences.", "ctrl+alt+p"),
    ];
    const cmds = state.config.commands;
    const report = { added: 0, replaced: 0, skipped: 0, renamed: [], hotkeys_dropped: 0 };
    for (const cmd of incoming) {
      const pos = cmds.findIndex((c) => c.id === cmd.id);
      if (pos >= 0 && mode === "skip") { report.skipped += 1; continue; }
      if (cmd.hotkey && cmds.some((c, i) => c.hotkey === cmd.hotkey && !(mode === "replace" && i === pos))) {
        cmd.hotkey = null;
        report.hotkeys_dropped += 1;
      }
      if (pos < 0) { cmds.push(cmd); report.added += 1; }
      else if (mode === "replace") { cmds[pos] = cmd; report.replaced += 1; }
      else {
        let n = 2;
        while (cmds.some((c) => c.id === cmd.id + "-" + n)) n += 1;
        const renamed = cmd.id + "-" + n;
        report.renamed.push([cmd.id, renamed]);
        cmds.push({ ...cmd, id: renamed, label: cmd.label + " (imported)" });
        report.added += 1;
      }
    }
    return report;
  }

  // Walk the states the native updater emits, ending where the real app
  // would relaunch.
  function runMockInstall() {
    const version = state.update && state.update.version || "0.8.0";
    const total = 18 * 1024 * 1024;
    const steps = [
      { state: "downloading", version, downloaded: total / 3, total },
      { state: "downloading", version, downloaded: total, total },
      { state: "waiting", version },
      { state: "installing", version },
    ];
    steps.forEach((step, i) => setTimeout(() => {
      state.update = step;
      emit("update-changed", clone(step));
    }, (i + 1) * 400));
  }

  function supervisorStatus() {
    if (!state.serveRunning) return { managed: false, pid: null, external: false, last_error: scenario === "errors" ? "serve exited with status 1" : null, child_status: null };
    if (state.serveExternal) return { managed: false, pid: null, external: true, last_error: null, child_status: null };
    return {
      managed: true,
      pid: 48213,
      external: false,
      last_error: null,
      child_status: { version: "0.7.0", id: "serve-48213", readiness: "ready", ax_trust: state.ax, generation: 1, external: false },
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
        if (scenario === "errors" || state.failSaves) throw "Could not write ~/.config/selara/config.toml: Permission denied (os error 13)";
        return applySection(args.section, args.value);
      case "save_config":
        state.config = clone(args.config);
        return null;
      case "config_path": return "~/.config/selara/config.toml";
      case "history_path": return "~/Library/Application Support/selara/history.jsonl";
      case "api_key_source": return keySource();
      case "store_api_key":
        if (!state.keychain.includes(args.kind)) state.keychain.push(args.kind);
        return keySource();
      case "clear_api_key":
        state.keychain = state.keychain.filter((kind) => kind !== args.kind);
        return keySource();
      case "cli_provider_status":
        if (args.kind === "cursor_cli") return { installed: false, version: null, binary: null, message: "cursor-agent was not found on PATH." };
        return { installed: true, version: "2.1.4", binary: "/opt/homebrew/bin/" + ({ claude_cli: "claude", open_code_cli: "opencode" }[args.kind] || "cli"), message: "CLI available. Sign in through the CLI before writing." };
      case "export_commands": return "~/Downloads/selara-commands.json";
      case "import_commands": return importPack(args.mode || "keep_both");
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
      case "clear_usage": state.usage = { ...emptyUsage(), providers: [] }; return clone(state.usage);
      case "try_command": return tryCommand(args);
      case "serve_status": return { running: state.serveRunning, pid: state.serveRunning ? 48213 : null, pidfile: "~/Library/Application Support/selara/serve.pid" };
      case "serve_supervisor_status": return supervisorStatus();
      case "serve_log": return state.log.slice();
      case "serve_start":
      case "serve_restart":
        if (scenario === "errors") throw "selara serve exited during startup: Accessibility permission is missing";
        state.serveRunning = true;
        state.serveExternal = false;
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
      // Like the backend: only a managed serve reports its grant.
      case "accessibility_status": return state.serveRunning && state.serveExternal ? "unknown" : state.ax;
      case "open_accessibility_settings": return null;
      case "app_version": return "0.7.0";
      case "bundled_codex_version": return "0.153.4";
      case "system_accent_color": return accent;
      case "confirm_action": return state.confirm !== false;
      case "update_status": return clone(state.update);
      case "check_for_updates":
        if (scenario === "errors") throw "Update check failed: network unreachable";
        state.update = state.update && state.update.state === "available" ? state.update : { state: "up_to_date", current: "0.7.0" };
        return clone(state.update);
      case "install_update": runMockInstall(); return null;
      case "set_shortcut_recording": return null;
      case "app_icon": return appIcon(args.app);
      case "running_apps": return clone(RUNNING_APPS);
      case "choose_app": return clone(state.chooseApp);
      case "copy_png":
        state.pngCopies.push(String(args.png_base64 || "").length);
        return null;
      case "save_png":
        state.pngSaves.push({ suggested_name: args.suggested_name, bytes: String(args.png_base64 || "").length });
        return state.saveCancelled ? null : "~/Downloads/" + args.suggested_name;
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
    const call = { cmd, args: clone(args), startedAt: performance.now(), finishedAt: null };
    calls.push(call);
    return new Promise((resolve, reject) => {
      setTimeout(() => {
        call.finishedAt = performance.now();
        try { resolve(handle(cmd, args)); } catch (e) { reject(e); }
      }, delay);
    });
  }

  async function listen(event, callback) {
    if (!listeners.has(event)) listeners.set(event, new Set());
    listeners.get(event).add(callback);
    return () => listeners.get(event).delete(callback);
  }

  // Minimal stand-in for @tauri-apps/api/menu: records the menu instead of
  // drawing it, so tests can pick an item with chooseMenuItem(text).
  let lastMenu = null;
  const menuApi = {
    Menu: {
      async new(opts) {
        const items = (opts && opts.items) || [];
        const menu = {
          closed: false,
          async popup(at) {
            lastMenu = { at: at || null, items, menu };
            mock.menu = items.map((item) => (item.item === "Separator" ? "-" : { text: item.text, enabled: item.enabled !== false }));
            mock.menuIds = items.filter((item) => item.id).map((item) => item.id);
          },
          async close() { menu.closed = true; mock.closedMenus += 1; },
        };
        return menu;
      },
    },
  };
  function chooseMenuItem(text) {
    if (lastMenu && lastMenu.menu.closed) throw new Error("the open menu was already closed");
    const item = lastMenu && lastMenu.items.find((i) => i.text === text);
    if (!item) throw new Error("no open menu item named " + text);
    if (item.enabled === false) throw new Error(text + " is disabled");
    lastMenu = null;
    return item.action && item.action(item.id);
  }
  class LogicalPosition { constructor(x, y) { this.x = x; this.y = y; } }

  window.__TAURI__ = { core: { invoke }, event: { listen }, menu: menuApi, dpi: { LogicalPosition } };
  const mock = { emit, calls, state, menu: null, menuIds: [], closedMenus: 0, chooseMenuItem };
  window.__selaraMock = mock;

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
