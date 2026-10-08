# Selara

[![CI](https://github.com/snowopsdev/selara/actions/workflows/ci.yml/badge.svg)](https://github.com/snowopsdev/selara/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Select text in any app, press a hotkey, and rewrite it with the LLM of your choice.

Selara is a writing assistant for macOS built on a Rust core. It reads your current selection, runs it through a prompt you control, and replaces the selected text with the completed result. Use your ChatGPT account through the bundled Codex runtime, connect Claude Code, Cursor, or OpenCode, or bring an API key for OpenAI-compatible endpoints, Anthropic, or OpenRouter. Local servers such as Ollama and LM Studio work too.

<table>
  <tr>
    <td width="50%"><img alt="Ink Sweep: the selected sentence in a mail draft is tinted while a Concise chip above it reads esc to cancel" src="docs/screenshots/readme/moment-sweep-working-light.png"></td>
    <td width="50%"><img alt="Ghost Diff: a review card under the selection shows the Concise rewrite as a word diff with Replace, Another take, Copy, and Discard keys" src="docs/screenshots/readme/moment-ghost-diff-light.png"></td>
  </tr>
  <tr>
    <td align="center"><sub>The selection shows progress while the model works</sub></td>
    <td align="center"><sub>Commands set to review show the change before replacing</sub></td>
  </tr>
</table>

Inspired by [theJayTea/WritingTools](https://github.com/theJayTea/WritingTools). Selara is a clean-room architecture, not a port.

## Features

- **Works wherever text is selected.** A global hotkey (default `ctrl+shift+space`) opens the custom-instruction popover. Configured commands are available from the menu bar and their own shortcuts. Selara reads a nonempty selection through macOS Accessibility and writes the finished result back over that selection.
- **Selection stays explicit.** If no text is selected, Selara shows **Select text first** and does not send a request or paste stale clipboard contents. Clipboard-assisted capture is used only when fresh copied text and the original target can both be verified.
- **Progress at the selection.** While a command runs, the selected text itself is tinted with a soft shimmer, and a small chip names the command with **esc to cancel**. On success a green afterglow sweeps over the replaced text with **Replaced · ⌘Z to undo**. Where an app can't report where the selection is (many Electron, web, and canvas editors), Selara shows its working orb beside the editor instead.
- **Review before replacing.** Turn it on for a command and its result appears as a word diff in a card under the selection instead of being pasted straight away. Press ↩ to replace, ⇥ for another take, ⌘C to copy, or Esc to discard. Anything you don't accept is kept in History, so a paid-for result is never lost.
- **Custom instruction at the selection.** The hotkey opens a small popover anchored to your selection, naming the app and the selection's length. Intent chips (Shorter, Warmer, Formal, Direct, Bullets, Fix only) build an editable instruction; toggle them with 1–6 in an empty field or ⌘1–⌘6 at any time. ↑ recalls the last ten instructions, and the instruction can be saved as a menu-bar command. The source app stays frontmost.
- **Every command replaces the selection.** Proofread, Rewrite, Friendly, Professional, Concise, Summary, Key Points, Table, Translate, and custom commands all write the completed result over the selection while retaining formatting requested by the prompt.
- **Your prompts.** Every command is a labeled prompt with its own icon and color. Edit the built-ins, add your own, duplicate one to make a variation, and drag them into the order the menu bar should use. Prompts can use `{{language}}` (your preferred language) and `{{app}}` (the app the selection came from); the built-in **Translate** command is `Translate the text to {{language}}`.
- **Try a prompt before using it.** Run any command on sample text from Settings and see the result as a word diff, without switching apps or pasting anything.
- **Record command shortcuts.** Click the shortcut control and press your chosen keys together. Selara captures the combination, checks for conflicts, and pauses its own shortcuts while recording. After saving, the shortcut runs the command on the current selection. Large or secret-shaped selections still require confirmation.
- **Excluded apps.** Pick password managers, terminals, or anything else from your running apps or Applications, or enter a bundle id or pattern, and the hotkeys do nothing there: no command, selection, or clipboard read.
- **Native undo and cancel.** Pressing Escape while a command is running discards its result instead of replacing anything. After a replacement, use the target app's native ⌘Z to restore the original text.
- **History and recovery.** The last 50 completed rewrites are kept locally with their original text, command, app, and replacement outcome, shown as inline word diffs grouped by day. If Selara cannot apply or verify an edit, the result stays in History with copy controls and a badge that says what happened, without opening a result popup or retrying an uncertain paste.
- **Provider connections.** A dedicated Providers screen groups ChatGPT (Codex), installed CLIs (Claude Code, Cursor, OpenCode), and API connections (OpenAI-compatible, Anthropic, OpenRouter). It shows CLI versions, the connection your commands use, and each connection's last request, median response time, and 30-day activity.
- **API model discovery.** Load the model list straight from the provider. A bad key or URL shows up right there, so it doubles as a connection test.
- **ChatGPT via Codex (experimental).** Reuse your existing ChatGPT login through a bundled native writing runtime. Credentials stay in Codex’s file or Keychain store; no separate Codex installation or Node is required.
- **Size limits.** A replacement caution, a soft warning, and a hard maximum, set on one ruler with page equivalents, so a stray select-all never sends 100k characters to a metered API. They apply to menu, custom-instruction, and shortcut runs alike.
- **Secret guard.** If the selection looks like it holds an API key, a private key block, a JWT, or a card number, Selara asks before sending it to a hosted provider; local servers are exempt.
- **Live config.** Everything lives in one TOML file. The `serve` process watches the config directory for changes and re-registers hotkeys as soon as the Settings app saves (with a 5-second poll as a fallback), while staying idle otherwise.
- **Menu-bar Settings app.** A Tauri tray app with Status, General, Providers, Commands, History, Usage, and Limits tabs. Status opens on a single **Ready** verdict, or a three-step setup on a fresh install. A single-line footer keeps the app version and update controls on the left and reports problems on the right. No Dock icon, closes to the tray.
- **Local usage, cost, and a shareable receipt.** The Usage tab and `selara usage` show requests and reported token counts for today, the last 30 days, and all time. The tab adds a 30-day chart, a breakdown by model and provider, and a receipt of your all-time writing that you can copy or save as a 1080×1350 image for social posts; it holds totals only, never your text. Costs are estimates for known models on their vendor's API; subscription, local, and custom endpoints remain unpriced. The ledger stays on your device.
- **Scriptable CLI.** `selara init`, `selara list-commands`, `selara run <command>`, `selara usage`, and `selara key set|clear|status` for pipelines and quick checks.
- **Keys in the keychain.** API keys go into the OS credential store (macOS Keychain) by default; `config.toml` stays free of secrets unless you choose otherwise.

## How it works

```mermaid
flowchart LR
  Shell["OS shell<br/>hotkey / selection / UI"] --> Core["selara-core<br/>commands + providers + config"]
  Core --> Providers["Codex / Claude Code / Cursor / OpenCode<br/>OpenAI-compatible / Anthropic / OpenRouter"]
  Platform["selara-platform<br/>traits"] --> Shell
```

| Crate | Role |
| --- | --- |
| `crates/selara-core` | Commands, config, providers |
| `crates/selara-platform` | Traits for selection / hotkey / clipboard (+ macOS backend) |
| `apps/selara` | CLI + macOS `serve` desktop shell |
| `apps/selara-desktop` | Tauri tray + Settings UI |

The `serve` shell registers the custom-instruction and command hotkeys, builds the menu-bar command list, reads the current selection, sends `{prompt, selection}` to the configured provider, and replaces the selection only after a complete successful response. The Settings app edits the same config file.

## Install (macOS)

Every GitHub Release ships a `Selara-<version>-macos-arm64.dmg` (the menu-bar app), a `selara-<version>-macos-arm64.tar.gz` (the CLI), and Homebrew files. With the tap configured:

```bash
brew tap snowopsdev/selara
brew install --cask selara      # menu-bar app
brew install selara             # CLI
```

Until the release is signed and notarized by Apple, macOS may report the app as damaged on first launch; clear the quarantine flag with `xattr -d com.apple.quarantine /Applications/Selara.app`. Only verified notarized releases remove this caveat. The first upgrade from v0.4.1 must be installed manually; see [release and update recovery](docs/release-updater.md). Building from source is described next.

## Quick start (macOS)

Building from source requires Rust 1.95 or newer and Xcode command-line tools, including Swift. The Settings app also needs Node.js. See [CONTRIBUTING.md](CONTRIBUTING.md) for full setup and platform requirements.

1. **Create the config** (once):

   ```bash
   cargo run -p selara -- init
   ```

2. **Choose a connection.** Open **Settings → Providers** to sign in with ChatGPT, select an installed CLI, or configure an API/local connection. Claude Code, Cursor, and OpenCode must be installed and signed in separately. For an API connection, store its key in the OS keychain (macOS Keychain):

   ```bash
   echo -n sk-... | cargo run -p selara -- key set
   ```

   Or export `SELARA_API_KEY`, which takes precedence for API connections, or paste the key into Providers, where "Store in the OS keychain" is on by default. Local servers such as Ollama accept any placeholder key. Lookup order is environment, keychain, then `provider.api_key` in `config.toml`. Codex and external CLI connections use their own account stores. macOS may ask for Keychain access again after a development rebuild.

3. **Grant Accessibility.** System Settings → Privacy & Security → Accessibility, then enable Terminal or iTerm (whichever runs `cargo run`) or the `selara` binary itself. macOS may prompt on first launch, and starting `serve` also triggers the prompt.

4. **Start the shell.** The Settings app (next section) starts `serve` for you, restarts it if it crashes, and can start at login (General → Start at login). From a terminal it also works on its own:

   ```bash
   cargo run -p selara -- serve
   ```

   The app never starts a second copy while one is running elsewhere. Accessibility must be granted to whatever runs `serve`: the app bundle when it is managed, or the terminal / sidecar binary under `apps/selara-desktop/src-tauri/binaries/` during `tauri dev`.

5. Select text in TextEdit, Notes, Mail, or anywhere else, then choose a command from the Selara menu bar. You can also assign a command shortcut in Settings, or press `ctrl+shift+space` to enter a custom instruction.

To open the Settings app:

```bash
cd apps/selara-desktop && npx tauri dev
```

It lives in the menu bar. Left-click the icon (or choose **Open Settings**) to show the window. Closing the window hides it. **Quit** exits.

Use the Tauri command rather than `cargo run -p selara-desktop`. A debug build loads the Vite dev server that `tauri dev` starts for it, so running the binary alone opens a blank window.

Orca worktree setup copies ignored `.env` and `.env.*` files from the main checkout, preserving any files already in the new worktree. For a worktree created with `git worktree add`, run `python3 scripts/copy-worktree-env.py` from its root. This is a one-time local copy; see [worktree environment files](CONTRIBUTING.md#worktree-environment-files).

## Settings app tour

Settings changes are saved to the same `config.toml`. General, Limits, and the command editor save each field as soon as you press Return or leave it, the way System Settings does, with a brief **✓ Saved** beside the field; an invalid value shakes and is not saved. Providers saves with its button. If `serve` is running, saved changes apply within about a second. Right-click a command or History entry for its actions, and use ↑/↓ to move between tabs in the sidebar. With Reduce Motion on, animations become short crossfades.

The screenshots below use example data.

### Status

<table>
  <tr>
    <td width="50%"><img alt="Status reading Ready with the custom-instruction shortcut as keycaps, a Try a command pad showing a Proofread word diff, and Shell, Accessibility, Provider, and Shortcuts chips" src="docs/screenshots/readme/settings-status-try.png"></td>
    <td width="50%"><img alt="Status on a fresh install: Welcome to Selara with three numbered steps to start the shell, allow Accessibility, and connect a model" src="docs/screenshots/readme/settings-status-setup.png"></td>
  </tr>
</table>

The tab the app opens on. When everything is in place it says **Ready** and shows the custom-instruction shortcut as keycaps; until a request has actually gone through it says **Ready · provider not yet tested**. On a fresh install it shows **Welcome to Selara** with three numbered steps instead (start the shell, allow Accessibility, connect a model), and each step advances as soon as it is done.

**Try it…** opens a pad that runs any command on sample text through your active connection and shows the result as a word diff. Nothing is pasted anywhere, but each run spends tokens and counts toward Usage.

Four chips summarize the details; select one to expand it:

- **Shell**: whether `selara serve` is running, with its pid. `serve` writes `serve.pid` next to the config file while it runs and removes it on exit; **Troubleshooting** shows that path and the Terminal command to run it yourself.
- **Accessibility**: the grant reported by Selara's managed `serve` process, with an **Open Accessibility settings** button when needed. A process started outside Settings shows **Can’t verify** without blocking Ready; restart it under Settings management to inspect its grant.
- **Provider**: the saved connection, model, endpoint or executable, and auth mode. **Check connection** lists API models, checks the Codex sign-in, or checks an external CLI's version. CLI version checks do not establish account or model access. The API key is never displayed.
- **Shortcuts**: the custom-instruction hotkey and every command that has its own shortcut.

**Refresh** re-reads the config and re-runs the checks.

### App version and updates

The footer spans the whole window on every tab. The left side shows the installed version, update state, and a button to check for updates. When a release is available, **Install and restart** appears there. Download progress and expandable details keep update errors and release notes accessible. The right side stays empty unless something goes wrong, such as a failed save; routine updates like **Saved** are announced to VoiceOver without taking space. Development builds may have automatic updates disabled.

### General

<img alt="General with the custom-instruction hotkey as keycaps, a Reply in language picker set to English, 1Password as an excluded app chip with its icon, and Start at login" src="docs/screenshots/readme/settings-general.png" width="820">

**Custom instruction hotkey** is a recorder: click it, then press the new keys together. Reserved macOS shortcuts and conflicts with command shortcuts are caught before saving.

**Reply in** is a language picker with native names; type to filter. The language fills `{{language}}` in prompts and is added to every prompt as a hint: the model replies in that language unless the command itself names an output language (a "Translate to French" command wins) or the selected text is clearly written in another one, in which case it keeps the text's language.

**Excluded apps** lists apps where the hotkeys should do nothing, each as a chip with its icon. **Add app…** lists the apps that are running, plus **Choose from Applications…** and **Enter Bundle ID or Pattern…**. Entries match the localized name (`1Password`) or bundle id (`com.apple.Terminal`), case-insensitively; end an entry with `*` to match a prefix (`com.apple.*`, `iTerm*`). When the frontmost app is on the list, `serve` logs the ignored press and neither opens a window nor reads the selection or clipboard.

**Start at login** opens Selara in the background when you log in.

### Providers

<img alt="Providers grouped into Accounts, Installed CLIs, and API keys, with OpenAI-compatible marked In use and its last request, median time sparkline, and 30-day totals above its settings" src="docs/screenshots/readme/settings-providers.png" width="820">

Connections are grouped into **Accounts** (ChatGPT through Codex), **Installed CLIs** (Claude Code, Cursor, OpenCode), and **API keys** (OpenAI-compatible, Anthropic, OpenRouter). Each row has one status line, and only the connection your commands actually use carries an **In use** pill.

Choose a connection to see its activity and settings. The top strip shows the last request, the median response time with a sparkline of recent requests (**No timing yet** until one is recorded), and the last 30 days of requests and estimated cost. **Use for Commands** saves the selected connection, enables it, and makes it the one your writing commands use; on the connection already in use, the button becomes **Save Changes** once you edit it. A failed save keeps your draft and shows an error beside the button.

Each connection has an **enable/disable switch** in its detail pane that saves immediately and retains its settings. Disabling the active connection stops new writing requests after config reload; it does not cancel a request already running or select a fallback. Enabling a different connection does not make it active until you choose **Use for Commands**. Selara keeps one saved configuration per provider and authentication mode; multiple named instances of the same connection are not supported yet.

**Codex** uses the bundled native writing runtime with your ChatGPT account (experimental). Sign in through the browser and refresh the model list to choose a model available to your account. The account address is masked by default. No separate Codex installation or Node runtime is needed. **Advanced** holds the optional shared Codex home path.

**Claude Code, Cursor, and OpenCode** use the account signed in through their installed CLI. Leave **CLI executable** blank for discovery, or enter a command name or absolute path. **Check CLI** verifies installation and reports the version; authentication and model access are checked when a writing command runs. Leave the model blank for the CLI default; OpenCode overrides use `provider/model` IDs.

Automatic discovery looks for `claude`, `cursor-agent`, and `opencode`. If your Cursor installation uses a different executable name, select that executable explicitly.

Cursor uses read-only ask mode with writing tools denied, but its configured local and team hooks still apply and may process the submitted text. OpenCode uses its authentication adapters without external plugins, skills, or custom endpoint configuration.

**OpenAI-compatible, Anthropic, and OpenRouter** use API keys. Type a model ID or press **Load models** to fetch the provider's list. For OpenAI-compatible connections, a **Preset** fills the base URL and key hint for OpenAI, Ollama, LM Studio, vLLM, Groq, Mistral, Gemini, Together, or DeepSeek; choose **Custom endpoint** for anything else. Model loading reports a connection error if the key or URL is wrong.

### Commands

<img alt="Command editor beside the command list: Proofread with its icon, prompt, Insert language and app pills, a recorded shortcut, and a Try pad showing the result as a word diff" src="docs/screenshots/readme/settings-commands-editor.png" width="820">

The prompts available from the Selara menu bar. Every command replaces the current selection. **List** shows the commands beside an inline editor; **Grid** shows them as cards next to a live **Menu bar preview**. The order is the menu bar's order: drag a card, or press ⌥↑ and ⌥↓ on a focused one. Search filters the list, and right-clicking a command offers **Duplicate** and **Delete…**. **More** opens the command-pack controls: **Export…** saves every command as a TOML pack (`[[commands]]` tables, the same shape as `config.toml`), and **Import…** merges a TOML or JSON pack back in. Choose how to handle existing command IDs before importing: keep both under a new ID, replace yours, or skip. Imported hotkeys that are already taken are dropped rather than duplicated.

<img alt="Commands as a grid of cards in dark mode beside a Menu bar preview listing them in the same order with their shortcuts" src="docs/screenshots/readme/settings-commands-grid-dark.png" width="820">

Select a command to edit it in place. Each field saves when you press Return (⌘↩ in the prompt) or leave it, and the list updates after the save succeeds. A failed save keeps your draft and names the command with **Retry**, even after you move on to another one.

- **Label and icon.** Click the tile beside the label to pick an icon and color. Commands without one get their first letter and a color derived from their id.
- **Prompt.** The **Insert** pills add `{{language}}` or `{{app}}` at the cursor.
- **Try.** Runs the prompt as currently written on a sample, taken from your History when there is one, and shows the result as a word diff. Press ⌘R to run it again. Like the Status pad, each run spends tokens and counts toward Usage.
- **Advanced.** The optional model override (same provider and key), app rules, and **Review before replacing**.

To assign a shortcut, click **Record shortcut** (or **Change** next to the current keys), then press and release the keys together. Escape cancels recording; **Clear** removes the assignment. Selara pauses its shortcuts during recording and restores them when recording ends or Settings loses focus. Conflicts with other commands or the custom-instruction shortcut are identified before saving. Standard copy, cut, paste, undo, and redo shortcuts are reserved.

**Review before replacing** shows that command's result as a word diff in a card under the selection, and replaces only when you press ↩. ⇥ asks for another take, ⌘C copies the result, and Esc discards it. Results you don't accept are saved in History as **Not applied**.

**Apps** narrows a command to the apps it belongs in: a menu action or shortcut is ignored outside those apps. The field is comma-separated app names or bundle ids (a trailing `*` matches by prefix), and leaving it empty — the default for every built-in command — offers the command everywhere. Custom instructions follow the global excluded-app rules.

```toml
[[commands]]
id = "reply"
label = "Draft reply"
kind = "replace"
prompt = "Draft a short reply to this message."
apps = ["Mail", "com.apple.Notes"]
glyph = "✉︎"        # optional: one character for the icon tile
color = "#5e5ce6"   # optional: tile color
review = true       # optional: show the diff card before replacing
```

### History

<img alt="History in dark mode grouped by day, each rewrite shown as an inline word diff, with Not applied and Check Notes badges on two entries and filter chips for Needs attention and each app" src="docs/screenshots/readme/settings-history-dark.png" width="820">

Each completed rewrite produced while `serve` runs is recorded, grouped by day: the command, the app the selection came from, the time, and an inline word diff of what changed. Results that aren't edits of the original, such as summaries and tables, show the result with **Show original** instead. Filter with **Needs attention** or by app, or search the labels, apps, and text.

**Copy** puts the result back on the clipboard. Its arrow, or right-clicking the entry, offers **Copy Result** and **Copy Original**. **Clear History…** deletes the whole list after a native confirmation. Historical records from older releases remain readable.

A badge appears only when something needs attention, and says what happened. **Not applied** means the completed result never reached the paste step, so the source app still has your original text. **Check <app>: paste may have worked** (Paste unverified) means a paste was attempted but Selara could not confirm the edit; check your document before copying that result. Selara keeps these results available without interrupting you with a popup or automatically retrying an uncertain paste.

Restoring is a clipboard copy on purpose: putting text back into the source app needs that app's focus and Accessibility, which the Settings window does not have. To restore a replacement in the source app, use that app's native ⌘Z.

Privacy: the list lives in `history.jsonl` in the config directory (`~/.config/selara/` by default), is kept readable only by you (mode 0600, re-applied on every append), holds the selected text and the results verbatim, and is trimmed to the newest 50 entries. Clear it from the tab, or delete the file, if you would rather not keep it around.

### Usage

<table>
  <tr>
    <td width="62%"><img alt="Usage with today, last 30 days, and all-time request tiles, a 30-day requests-per-day bar chart, and a model share bar" src="docs/screenshots/readme/settings-usage.png"></td>
    <td width="38%"><img alt="The shareable Writing receipt image: rewrites, words polished, time saved, favourite model, estimated API cost, and cost per rewrite" src="docs/screenshots/readme/usage-receipt-share.jpg"></td>
  </tr>
</table>

Usage has its own navigation item. Tiles show requests and estimated cost for today (your local calendar day), the last 30 days, and all time. A bar chart shows requests per day for the last 30 days, and a share bar splits activity by model and provider. **Details** holds the full tables: input and output tokens per window, and all-time activity by model, ordered by request count.

**Your receipt** sums up all-time use: rewrites, words polished, time saved, favourite model, and cost per rewrite, with the assumptions behind the estimates footnoted. **Share…** renders it as a 1080×1350 image to **Copy Image** or **Save Image…**. The image contains totals only, never any text you selected or wrote.

Token counts depend on what the provider reports; missing metadata is identified instead of counted as zero. Costs are estimates in USD for known models on their vendor's own API, not a bill. Subscription requests and unpriced endpoints are excluded from cost totals and shown as **n/a** when no estimate is available.

**Refresh** reloads the local ledger. **Clear Usage Data…** clears `usage.jsonl` without changing settings or writing history. Usage data is stored beside the config and is never sent elsewhere by Selara.

### Limits

<img alt="Limits in dark mode: a single size ruler with markers for replace caution at 4,000, soft warning at 8,000, and hard maximum at 100,000 characters with page equivalents, and the secret guard switch" src="docs/screenshots/readme/settings-limits-dark.png" width="820">

Guard rails for large selections, set on one ruler. Its three markers can't cross, each shows its page equivalent (about 3,000 characters per page), and dragging a marker past the end means no limit. Arrow keys move a focused marker; double-click or Option-click it to type an exact number, or use the fields below the ruler. Changes save immediately; **Reset to Defaults** restores the standard values. Setting a value to `0` disables it. The replacement caution, soft warning, and secret guard are compact confirmations shown before a request or replacement. A command is never sent past the hard maximum.

| Setting | Default | Effect |
| --- | --- | --- |
| Replace caution | 4000 chars | Ask again before overwriting a large selection |
| Soft warn | 8000 chars | Ask before sending a large selection |
| Hard max | 100000 chars | Refuse anything larger |
| Secret guard | on | Ask before sending secret-shaped text (API keys, private keys, JWTs, card numbers) to a hosted provider; local servers (`localhost`, `.local`, private IPs) are exempt |

## Providers and config

Default config path: `~/.config/selara/config.toml`. The file is written atomically (temp file + rename) with owner-only permissions (`0600`), and the Settings app saves one tab at a time, so saving one section preserves changes to other sections and commands saved from the instruction dialog. A `schema_version` key records the file format (currently `2`); versionless files are read as version 1 and migrated in memory. Legacy popup commands are normalized to replacement behavior while command ids, prompts, models, app filters, and valid shortcuts are preserved.

| `kind` | Wire format | Default `base_url` |
|---|---|---|
| `open_ai_compatible` | OpenAI `/chat/completions` (OpenAI, Ollama, LM Studio, vLLM, ...) | `https://api.openai.com/v1` |
| `anthropic` | Anthropic Messages API | `https://api.anthropic.com` |
| `open_router` | OpenAI-compatible via OpenRouter | `https://openrouter.ai/api/v1` |
| `claude_cli` | Installed Claude Code CLI | Not used |
| `cursor_cli` | Installed Cursor Agent CLI | Not used |
| `open_code_cli` | Installed OpenCode CLI | Not used |

For API providers, leave `base_url` empty to use the default. Old configs with `kind = "ollama"` still load as `open_ai_compatible`. Codex uses `kind = "open_ai_compatible"` with `auth = "chatgpt"` and ignores `base_url`.

`[provider]` is the active connection. `enabled` defaults to `true` for existing configurations. Settings retains other connections in `provider_connections`, keyed by kind and auth mode, so changing the active connection does not discard their saved settings.

Prompt placeholders: `{{language}}` expands to the configured language and `{{app}}` to the name of the app the selection came from (CLI runs use "the current application"). Unknown placeholders are left as written; the selected text itself is always sent as the message, so it needs no placeholder. Configs that list their own `commands` do not gain the new built-in **Translate** command automatically; add a command with the prompt `Translate the text to {{language}}. Return only the translation.` to get it.

Anthropic:

```toml
[provider]
kind = "anthropic"
base_url = ""
model = "claude-opus-5"
```

Ollama:

```toml
[provider]
kind = "open_ai_compatible"
base_url = "http://localhost:11434/v1"
model = "llama3.1:8b"
api_key = "ollama"
```

Environment and paths:

- `SELARA_CONFIG_DIR` overrides the config directory (falls back to the legacy `WRITING_TOOLS_CONFIG_DIR`).
- For API connections, `SELARA_API_KEY` takes precedence (falls back to the legacy `WRITING_TOOLS_API_KEY`); next comes the OS keychain entry (`selara key set|clear|status`, service `dev.snowops.selara`), then `provider.api_key`. Codex and external CLI adapters use their own authentication stores.
- `selara --config <path>` overrides the file for a single CLI invocation.
- **Migration:** on first load, if the Selara config is missing but `~/.config/writing-tools/config.toml` exists, it is copied into the Selara path (a one-time message is printed).
- ChatGPT via Codex uses the bundled native writing runtime. Set an absolute `provider.codex_home` in Settings if your shared login is outside `~/.codex`; it takes precedence over `CODEX_HOME`. Remove legacy `CODEX_BIN` and `CODEX_AUTH_JSON` overrides. Signing out affects that shared Codex home, including other Codex clients.

Claude Code example (Cursor and OpenCode use `cursor_cli` and `open_code_cli` respectively):

```toml
[provider]
kind = "claude_cli"
enabled = true
base_url = ""
model = "" # Use the CLI's default model.
# cli_binary = "/absolute/path/to/claude" # Optional; otherwise discovered.
```

## The `serve` shell in detail

### Hotkey

The custom-instruction shortcut defaults to `ctrl+shift+space`. Plain `ctrl+space` often conflicts with macOS Input Sources and Spotlight, so the default avoids it. Override it in the General tab or in config:

```toml
hotkey = "option+space"
# or: "cmd+shift+w", "ctrl+shift+space", …
excluded_apps = ["1Password", "com.apple.Terminal"]   # optional; hotkeys are ignored in these apps
```

Supported tokens: `ctrl`/`control`, `shift`, `alt`/`option`, `cmd`/`command`/`super`, plus a key: `space`, `a`–`z`, `0`–`9`, `enter`, `tab`, `escape`; function keys `f1`–`f12`; arrows `up`/`down`/`left`/`right`; editing keys `backspace`, `delete`, `home`, `end`, `pageup`, `pagedown`; and the punctuation characters `-` `=` `[` `]` `;` `'` `,` `.` `/` `` ` `` `\`. Tokens are case-insensitive and may be padded with spaces. The same grammar applies to per-command hotkeys.

### Replace strategy

Selara captures the actual nonempty selection and its source application before opening any confirmation or progress UI. After a complete provider response, it revalidates the original app, window, element, and selection before using the source app's normal paste operation once. If Selara's own UI holds focus, it can return focus to the source app. It does not paste while an unrelated app is frontmost. A changed or unverifiable target is rejected before pasting. If the paste itself cannot be verified, the result is marked in History and is not retried. Use the source app's native ⌘Z to undo an applied replacement.

### Known limitations

- The paste fallback snapshots the whole pasteboard (every type, images included) and only restores it if nothing else was copied in between, so a copy you make during that window wins and the pre-Selara clipboard is dropped.
- Some apps (Electron, browsers, certain rich-text fields) do not expose a stable selection or consume paste slowly. Selara refuses an unverified replacement and never retries an uncertain paste automatically.
- While a command runs, Selara tints the selection itself, using the line bounds the app reports through Accessibility. When those bounds are missing or implausible (no bounds, web areas, more than 12 lines, off-screen, or larger than the editor), it falls back to a transparent working orb beside the editor near the selection, or offset from the cursor position captured when the command started. Click the chip, or hover over the orb to reveal its cancel button, to cancel; Escape works too. The success afterglow, or the orb's brief checkmark, appears only after verified replacement. The orb uses the SwiftUI [ThinkingOrbsKit](apps/selara/native/ThinkingOrbsKit/UPSTREAM.md) working animation on macOS 12+; macOS 11 uses a native spinner. Try the [standalone preview](examples/native-progress-preview.md) without running a writing command.
- If replacement cannot be verified, Selara saves the completed rewrite in **Settings → History** without opening a dialog or retrying the paste. Entries marked **Not applied** did not reach the paste step; for **Paste unverified**, check your document before copying the saved result.
- Global hotkeys need the `serve` process running. The Settings app keeps it running and can register itself as a login item; a bare terminal `serve` still stops when the terminal closes.
- If a hotkey fails to register or never fires, pick another chord.

## CLI

```bash
cargo run -p selara -- init
cargo run -p selara -- list-commands
cargo run -p selara -- run proofread --text "Their going to the store tommorow."
cargo run -p selara -- run summary --text "$(pbpaste)"
```

`run` reads stdin when `--text` is omitted, returns the completed result on stdout, and `--instruct` appends a one-off instruction to the command's prompt. `--no-stream` is accepted for compatibility. `selara usage` prints the local usage ledger as a table (tokens in/out per window and per model, with estimated costs).

## Status

**Available:** configurable writing commands with per-command review, recorded shortcuts, a custom-instruction popover anchored to the selection, Codex and external CLI connections, API/local providers, progress drawn over the selection, History with word diffs and replacement outcomes, a Usage view with a shareable receipt, and version/update controls available throughout Settings. The Settings app manages `serve`, supports starting at login, and reloads saved configuration.

**Next:**

Windows and Linux desktop shells implementing the same platform traits. The portable core and CLI are already covered by Linux CI.

## Contributing, security, license

- [CONTRIBUTING.md](CONTRIBUTING.md) — clone → test → PR
- [SECURITY.md](SECURITY.md) — how we handle security
- [Report a vulnerability](https://github.com/snowopsdev/selara/security/advisories/new) — private GitHub Security Advisory (preferred)
- [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) — Contributor Covenant 2.1
- [LICENSE](LICENSE) — MIT

CI runs tests on Linux and compiles the macOS shell on `macos-latest`. macOS Accessibility, global hotkeys, `serve`, and the Tauri UI still need local macOS testing when those areas change.
