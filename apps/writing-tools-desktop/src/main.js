import { invoke } from "@tauri-apps/api/core";

let config = null;
let editingId = null;

const $ = (sel) => document.querySelector(sel);
const statusEl = () => $("#save-status");

function setStatus(msg) {
  statusEl().textContent = msg || "";
}

function showSection(id) {
  document.querySelectorAll(".nav-item").forEach((b) => {
    b.classList.toggle("active", b.dataset.section === id);
  });
  document.querySelectorAll(".section").forEach((s) => {
    s.classList.toggle("active", s.id === `section-${id}`);
  });
}

document.querySelectorAll(".nav-item").forEach((btn) => {
  btn.addEventListener("click", () => showSection(btn.dataset.section));
});

function kindLabel(k) {
  return k === "popup" ? "popup" : "replace";
}

function renderGeneral() {
  const s = $("#section-general");
  s.innerHTML = `
    <h1>General</h1>
    <p class="lead">Language and the global shortcut for the writing picker. Changes save to your config file.</p>
    <div class="card">
      <label for="language">Default language</label>
      <input id="language" value="${escapeAttr(config.language || "en")}" placeholder="en" />
      <p class="hint">Used so the model can respect your preferred language (e.g. en, es, fr).</p>
      <label for="hotkey">Picker hotkey</label>
      <input id="hotkey" value="${escapeAttr(config.hotkey || "ctrl+shift+space")}" placeholder="ctrl+shift+space" />
      <p class="hint">Examples: ctrl+shift+space, option+space, cmd+shift+w. With `writing-tools serve` running, changes apply within about a second.</p>
      <div class="actions">
        <button class="btn" id="save-general">Save General</button>
      </div>
    </div>
  `;
  $("#save-general").onclick = async () => {
    config.language = $("#language").value.trim() || "en";
    config.hotkey = $("#hotkey").value.trim() || "ctrl+shift+space";
    await saveConfig();
  };
}

function renderModels() {
  const p = config.provider || {};
  const s = $("#section-models");
  s.innerHTML = `
    <h1>Models</h1>
    <p class="lead">Connect an API key for the active provider. Subscription sign-in comes later.</p>
    <div class="card">
      <label for="provider-kind">Provider</label>
      <select id="provider-kind">
        <option value="openai_compatible">OpenAI-compatible (OpenAI, Groq, etc.)</option>
        <option value="anthropic">Anthropic</option>
        <option value="ollama">Ollama (local)</option>
        <option value="gemini">Google Gemini</option>
      </select>
      <div class="row">
        <div>
          <label for="model">Model</label>
          <input id="model" value="${escapeAttr(p.model || "")}" />
        </div>
        <div>
          <label for="base_url">Base URL</label>
          <input id="base_url" value="${escapeAttr(p.base_url || "")}" />
        </div>
      </div>
      <label for="api_key">API key</label>
      <input id="api_key" type="password" value="${escapeAttr(p.api_key || "")}" placeholder="sk-… or leave empty to use WRITING_TOOLS_API_KEY" />
      <p class="hint">Stored in ~/.config/writing-tools/config.toml. Prefer env WRITING_TOOLS_API_KEY when possible.</p>
      <div class="actions">
        <button class="btn" id="save-models">Save Models</button>
      </div>
    </div>
    <div class="soon">
      <strong>Subscription / OAuth</strong> — ChatGPT or Claude subscription sign-in is planned. API keys work today.
    </div>
  `;
  const kind = p.kind || "openai_compatible";
  $("#provider-kind").value = kind;
  $("#provider-kind").onchange = () => {
    const k = $("#provider-kind").value;
    if (k === "anthropic" && !$("#base_url").value) {
      $("#base_url").value = "https://api.anthropic.com";
      if (!$("#model").value) $("#model").value = "claude-sonnet-4-20250514";
    }
    if (k === "openai_compatible" && (!$("#base_url").value || $("#base_url").value.includes("anthropic"))) {
      $("#base_url").value = "https://api.openai.com/v1";
      if (!$("#model").value || $("#model").value.includes("claude")) $("#model").value = "gpt-4o-mini";
    }
    if (k === "ollama") {
      $("#base_url").value = "http://localhost:11434/v1";
      if (!$("#model").value) $("#model").value = "llama3.2";
    }
    if (k === "gemini") {
      $("#base_url").value = "";
      if (!$("#model").value) $("#model").value = "gemini-2.0-flash";
    }
  };
  $("#save-models").onclick = async () => {
    config.provider = {
      kind: $("#provider-kind").value,
      model: $("#model").value.trim(),
      base_url: $("#base_url").value.trim(),
      api_key: $("#api_key").value.trim() || null,
    };
    await saveConfig();
  };
}

function renderCommands() {
  const s = $("#section-commands");
  const list = (config.commands || [])
    .map(
      (c) => `
      <div class="cmd-item" data-id="${escapeAttr(c.id)}">
        <div>
          <h3>${escapeHtml(c.label)} <span class="badge">${kindLabel(c.kind)}</span>${c.hotkey ? ` <span class="badge">${escapeHtml(c.hotkey)}</span>` : ""}</h3>
          <p>${escapeHtml(c.prompt)}</p>
        </div>
        <div class="actions">
          <button class="btn secondary" data-act="edit">Edit</button>
          <button class="btn secondary" data-act="dup">Duplicate</button>
          <button class="btn danger" data-act="del">Delete</button>
        </div>
      </div>`
    )
    .join("");

  const editing = editingId
    ? (config.commands || []).find((c) => c.id === editingId)
    : null;

  s.innerHTML = `
    <h1>Commands</h1>
    <p class="lead">Each command is a prompt. No limit on how many you create. Duplicate to make variations.</p>
    <div class="card">
      <h3 style="margin-top:0">${editing ? "Edit command" : "New command"}</h3>
      <div class="row">
        <div>
          <label>Label</label>
          <input id="cmd-label" value="${escapeAttr(editing?.label || "")}" />
        </div>
        <div>
          <label>Kind</label>
          <select id="cmd-kind">
            <option value="replace">Replace selection</option>
            <option value="popup">Popup result</option>
          </select>
        </div>
      </div>
      <label>Prompt</label>
      <textarea id="cmd-prompt">${escapeHtml(editing?.prompt || "")}</textarea>
      <label>Command hotkey (optional)</label>
      <input id="cmd-hotkey" value="${escapeAttr(editing?.hotkey || "")}" placeholder="e.g. ctrl+shift+p" />
      <p class="hint">Runs this command directly on the current selection (skips the picker). Leave blank for none.</p>
      <div class="actions">
        <button class="btn" id="cmd-save">${editing ? "Update" : "Add command"}</button>
        ${editing ? '<button class="btn secondary" id="cmd-cancel">Cancel</button>' : ""}
      </div>
    </div>
    <div class="cmd-list">${list || "<p class='hint'>No commands yet.</p>"}</div>
  `;
  if (editing) $("#cmd-kind").value = editing.kind;
  $("#cmd-save").onclick = async () => {
    const label = $("#cmd-label").value.trim();
    const prompt = $("#cmd-prompt").value.trim();
    const kind = $("#cmd-kind").value;
    const hotkeyRaw = $("#cmd-hotkey").value.trim();
    const hotkey = hotkeyRaw ? hotkeyRaw : null;
    if (!label || !prompt) {
      setStatus("Label and prompt are required");
      return;
    }
    if (editingId) {
      const i = config.commands.findIndex((c) => c.id === editingId);
      if (i >= 0) {
        config.commands[i] = { ...config.commands[i], label, prompt, kind, hotkey };
      }
      editingId = null;
    } else {
      const id = slug(label) + "-" + Math.random().toString(36).slice(2, 7);
      config.commands.push({ id, label, prompt, kind, hotkey });
    }
    await saveConfig();
    renderCommands();
  };
  const cancel = $("#cmd-cancel");
  if (cancel) {
    cancel.onclick = () => {
      editingId = null;
      renderCommands();
    };
  }
  s.querySelectorAll(".cmd-item").forEach((el) => {
    const id = el.dataset.id;
    el.querySelector('[data-act="edit"]').onclick = () => {
      editingId = id;
      renderCommands();
    };
    el.querySelector('[data-act="dup"]').onclick = async () => {
      const src = config.commands.find((c) => c.id === id);
      if (!src) return;
      config.commands.push({
        ...src,
        id: src.id + "-copy-" + Math.random().toString(36).slice(2, 6),
        label: src.label + " copy",
        hotkey: null,
      });
      await saveConfig();
      renderCommands();
    };
    el.querySelector('[data-act="del"]').onclick = async () => {
      if (!confirm(`Delete “${id}”?`)) return;
      config.commands = config.commands.filter((c) => c.id !== id);
      if (editingId === id) editingId = null;
      await saveConfig();
      renderCommands();
    };
  });
}

function renderLimits() {
  const L = config.limits || {};
  const s = $("#section-limits");
  s.innerHTML = `
    <h1>Limits</h1>
    <p class="lead">Gentle rails against huge accidental pastes. Set any value to 0 for unlimited.</p>
    <div class="card">
      <label>Soft warn (chars)</label>
      <input id="soft_warn" type="number" min="0" value="${L.soft_warn_chars ?? 8000}" />
      <label>Hard max (chars)</label>
      <input id="hard_max" type="number" min="0" value="${L.hard_max_chars ?? 100000}" />
      <label>Replace caution (chars)</label>
      <input id="replace_warn" type="number" min="0" value="${L.replace_warn_chars ?? 4000}" />
      <div class="actions">
        <button class="btn" id="save-limits">Save Limits</button>
        <button class="btn secondary" id="reset-limits">Reset defaults</button>
      </div>
    </div>
  `;
  $("#save-limits").onclick = async () => {
    config.limits = {
      soft_warn_chars: num($("#soft_warn").value),
      hard_max_chars: num($("#hard_max").value),
      replace_warn_chars: num($("#replace_warn").value),
    };
    await saveConfig();
  };
  $("#reset-limits").onclick = async () => {
    config.limits = {
      soft_warn_chars: 8000,
      hard_max_chars: 100000,
      replace_warn_chars: 4000,
    };
    await saveConfig();
    renderLimits();
  };
}

async function saveConfig() {
  try {
    await invoke("save_config", { config });
    setStatus("Saved");
  } catch (e) {
    setStatus(String(e));
  }
}

async function load() {
  config = await invoke("get_config");
  renderGeneral();
  renderModels();
  renderCommands();
  renderLimits();
  setStatus("Loaded");
}

function num(v) {
  const n = Number(v);
  return Number.isFinite(n) && n >= 0 ? Math.floor(n) : 0;
}
function slug(s) {
  return s.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || "command";
}
function escapeHtml(s) {
  return String(s ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;");
}
function escapeAttr(s) {
  return escapeHtml(s).replaceAll('"', "&quot;");
}

load().catch((e) => setStatus(String(e)));
