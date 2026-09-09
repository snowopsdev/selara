use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::providers::{CompletionRequest, DeltaSink, LlmProvider};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandKind {
    Replace,
    Popup,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WritingCommand {
    pub id: String,
    pub label: String,
    pub kind: CommandKind,
    /// Instruction sent to the model with the selected/input text.
    pub prompt: String,
    /// Optional global shortcut that runs this command directly (skip picker).
    #[serde(default)]
    pub hotkey: Option<String>,
    /// Optional model id that overrides `provider.model` for this command
    /// only (same provider and credentials), e.g. a cheap local model for
    /// Proofread and a frontier model for Rewrite.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

pub fn builtin_commands() -> Vec<WritingCommand> {
    vec![
        WritingCommand {
            id: "proofread".into(),
            label: "Proofread".into(),
            kind: CommandKind::Replace,
            prompt: "Proofread the text. Fix grammar, spelling, and punctuation only. Keep meaning and voice. Return only the corrected text.".into(),
            hotkey: None,
            model: None,
        },
        WritingCommand {
            id: "rewrite".into(),
            label: "Rewrite".into(),
            kind: CommandKind::Replace,
            prompt: "Rewrite the text for clarity and flow. Keep the original meaning. Return only the rewritten text.".into(),
            hotkey: None,
            model: None,
        },
        WritingCommand {
            id: "friendly".into(),
            label: "Friendly".into(),
            kind: CommandKind::Replace,
            prompt: "Rewrite the text in a warm, friendly tone. Return only the rewritten text.".into(),
            hotkey: None,
            model: None,
        },
        WritingCommand {
            id: "professional".into(),
            label: "Professional".into(),
            kind: CommandKind::Replace,
            prompt: "Rewrite the text in a clear, professional tone. Return only the rewritten text.".into(),
            hotkey: None,
            model: None,
        },
        WritingCommand {
            id: "concise".into(),
            label: "Concise".into(),
            kind: CommandKind::Replace,
            prompt: "Make the text more concise without losing key meaning. Return only the rewritten text.".into(),
            hotkey: None,
            model: None,
        },
        WritingCommand {
            id: "summary".into(),
            label: "Summary".into(),
            kind: CommandKind::Popup,
            prompt: "Summarize the text clearly in markdown. Use short paragraphs or bullets as needed.".into(),
            hotkey: None,
            model: None,
        },
        WritingCommand {
            id: "key_points".into(),
            label: "Key Points".into(),
            kind: CommandKind::Popup,
            prompt: "Extract the key points as a markdown bullet list.".into(),
            hotkey: None,
            model: None,
        },
        WritingCommand {
            id: "table".into(),
            label: "Table".into(),
            kind: CommandKind::Popup,
            prompt: "Convert the useful information in the text into a markdown table.".into(),
            hotkey: None,
            model: None,
        },
        WritingCommand {
            id: "translate".into(),
            label: "Translate".into(),
            kind: CommandKind::Replace,
            prompt: "Translate the text to {{language}}. Return only the translation.".into(),
            hotkey: None,
            model: None,
        },
    ]
}

/// A shareable set of commands: `commands = [...]` / `[[commands]]` in TOML, or
/// a JSON array / `{ "commands": [...] }`. This is the same shape as the
/// `commands` table in `config.toml`, so a config file is itself a valid pack.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CommandPack {
    #[serde(default)]
    pub commands: Vec<WritingCommand>,
}

/// Parse a pack from JSON or TOML text.
pub fn parse_command_pack(text: &str) -> Result<Vec<WritingCommand>, CoreError> {
    let trimmed = text.trim_start();
    if trimmed.starts_with('[') || trimmed.starts_with('{') {
        if let Ok(list) = serde_json::from_str::<Vec<WritingCommand>>(trimmed) {
            return Ok(list);
        }
        if let Ok(pack) = serde_json::from_str::<CommandPack>(trimmed) {
            return Ok(pack.commands);
        }
    }
    match toml::from_str::<CommandPack>(text) {
        Ok(pack) if !pack.commands.is_empty() => Ok(pack.commands),
        Ok(_) => Err(CoreError::Config(
            "command pack contains no commands (expected `[[commands]]` in TOML or a JSON array)"
                .into(),
        )),
        Err(e) => Err(CoreError::Config(format!(
            "could not read command pack as JSON or TOML: {e}"
        ))),
    }
}

/// Render commands as a TOML pack (`[[commands]]` tables).
pub fn render_command_pack(commands: &[WritingCommand]) -> Result<String, CoreError> {
    let pack = CommandPack {
        commands: commands.to_vec(),
    };
    toml::to_string_pretty(&pack).map_err(|e| CoreError::Config(e.to_string()))
}

/// What to do when an imported command has the same id as an existing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeMode {
    /// Keep the existing command and add the import under a new id.
    KeepBoth,
    /// Overwrite the existing command in place (its position is kept).
    Replace,
    /// Leave the existing command alone and drop the import.
    Skip,
}

/// Outcome of [`merge_commands`], for the UI's status line.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct MergeReport {
    pub added: usize,
    pub replaced: usize,
    pub skipped: usize,
    /// `(imported id, id it was stored under)` when `KeepBoth` had to rename.
    pub renamed: Vec<(String, String)>,
    /// Imported hotkeys dropped because another command already uses them.
    pub hotkeys_dropped: usize,
}

/// Canonical identity of a hotkey spec, for telling whether two spellings name
/// the same binding. Mirrors `parse_hotkey` in `selara-platform`: `+`-separated
/// tokens trimmed and lowercased, modifier and key aliases folded together,
/// modifier order ignored, and the last key token wins. `None` when the spec
/// names no key at all, which is what `parse_hotkey` rejects.
///
/// Core cannot call `parse_hotkey` directly: it is behind the macOS-only
/// `global_hotkey` dependency. The two normalizations must agree, so any alias
/// added to `parse_hotkey` belongs here too.
pub fn canonical_hotkey(spec: &str) -> Option<String> {
    // ctrl, alt, shift, meta.
    let mut mods = [false; 4];
    let mut key: Option<String> = None;
    for part in spec.split('+').map(|s| s.trim().to_ascii_lowercase()) {
        if part.is_empty() {
            continue;
        }
        match part.as_str() {
            "ctrl" | "control" | "control_l" | "control_r" => mods[0] = true,
            "alt" | "option" | "opt" => mods[1] = true,
            "shift" => mods[2] = true,
            "cmd" | "command" | "super" | "meta" | "win" => mods[3] = true,
            _ => key = Some(canonical_hotkey_key(&part)),
        }
    }
    let key = key?;
    let mut out = String::new();
    for (on, name) in mods.iter().zip(["ctrl", "alt", "shift", "meta"]) {
        if *on {
            out.push_str(name);
            out.push('+');
        }
    }
    out.push_str(&key);
    Some(out)
}

/// Fold the key aliases `parse_hotkey` accepts onto a single spelling.
fn canonical_hotkey_key(token: &str) -> String {
    match token {
        "return" => "enter",
        "esc" => "escape",
        "del" => "delete",
        "pgup" => "pageup",
        "pgdn" => "pagedown",
        "arrowup" => "up",
        "arrowdown" => "down",
        "arrowleft" => "left",
        "arrowright" => "right",
        other => other,
    }
    .to_string()
}

/// Merge `incoming` into `existing` according to `mode`. Hotkeys that would
/// collide with a command already in the list, or with one of `reserved` (the
/// picker and undo bindings, which `reregister_all` registers first), are
/// dropped rather than duplicated, so a reload of `serve` cannot fail on a
/// duplicate binding.
pub fn merge_commands(
    existing: &mut Vec<WritingCommand>,
    incoming: Vec<WritingCommand>,
    mode: MergeMode,
    reserved: &[&str],
) -> MergeReport {
    let mut report = MergeReport::default();
    let reserved: Vec<String> = reserved
        .iter()
        .filter_map(|spec| canonical_hotkey(spec))
        .collect();
    for mut cmd in incoming {
        let pos = existing.iter().position(|c| c.id == cmd.id);
        // A skipped import changes nothing that gets saved, so decide that
        // before any hotkey accounting: it must not report a dropped hotkey.
        if pos.is_some() && mode == MergeMode::Skip {
            report.skipped += 1;
            continue;
        }
        // Hotkey collisions: compare canonical identities, because
        // `reregister_all` rejects duplicates by parsed hotkey id, not by
        // spelling. Everything is compared except the command this one is
        // about to replace.
        if let Some(hk) = cmd.hotkey.as_deref().and_then(canonical_hotkey) {
            let replacing = pos.filter(|_| mode == MergeMode::Replace);
            let taken = reserved.contains(&hk)
                || existing.iter().enumerate().any(|(i, c)| {
                    Some(i) != replacing
                        && c.hotkey.as_deref().and_then(canonical_hotkey).as_deref()
                            == Some(hk.as_str())
                });
            if taken {
                cmd.hotkey = None;
                report.hotkeys_dropped += 1;
            }
        }
        match (pos, mode) {
            (None, _) => {
                existing.push(cmd);
                report.added += 1;
            }
            (Some(i), MergeMode::Replace) => {
                existing[i] = cmd;
                report.replaced += 1;
            }
            (Some(_), MergeMode::Skip) => unreachable!("skips return above"),
            (Some(_), MergeMode::KeepBoth) => {
                let original = cmd.id.clone();
                let mut n = 2;
                while existing.iter().any(|c| c.id == format!("{original}-{n}")) {
                    n += 1;
                }
                cmd.id = format!("{original}-{n}");
                cmd.label = format!("{} (imported)", cmd.label);
                report.renamed.push((original, cmd.id.clone()));
                existing.push(cmd);
                report.added += 1;
            }
        }
    }
    report
}

/// Values a prompt can reference with `{{name}}` placeholders.
///
/// `{{language}}` is the preferred language from config; `{{app}}` is the
/// frontmost application the selection came from (`serve` only). The selected
/// text itself is always sent as the user message, so prompts do not need a
/// placeholder for it. Unknown placeholders are left as written.
#[derive(Debug, Clone, Copy, Default)]
pub struct PromptVars<'a> {
    pub language: Option<&'a str>,
    pub app: Option<&'a str>,
}

const APP_FALLBACK: &str = "the current application";
const LANGUAGE_FALLBACK: &str = "the same language as the text";

/// Replace `{{language}}` / `{{app}}` (whitespace inside the braces is
/// allowed) with their values, or a neutral fallback when unknown.
pub fn substitute_prompt_vars(template: &str, vars: PromptVars<'_>) -> String {
    let language = vars
        .language
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .unwrap_or(LANGUAGE_FALLBACK);
    let app = vars
        .app
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .unwrap_or(APP_FALLBACK);

    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let key = after[..end].trim();
                match key {
                    "language" => out.push_str(language),
                    "app" => out.push_str(app),
                    _ => out.push_str(&rest[start..start + 2 + end + 2]),
                }
                rest = &after[end + 2..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Output contract appended to the system prompt of every `Replace` command.
/// Replace output is written verbatim over the user's selection, so any chat
/// framing the model adds would land in their document.
pub const REPLACE_OUTPUT_RULES: &str = "Reply with the transformed text only. Do not add an \
introduction, explanation, or closing remark. Do not wrap the reply in quotes or a code fence. \
Preserve the original's line breaks and leading and trailing whitespace unless the instruction \
asks otherwise. If the text is already correct, return it unchanged.";

/// Assemble the system prompt: the command's prompt, then (for `Replace`
/// commands) the output rules, then the preferred language from config (blank
/// means no hint), then any one-off instruction.
pub fn build_system_prompt(
    command: &WritingCommand,
    custom_instruction: Option<&str>,
    language: Option<&str>,
) -> String {
    build_system_prompt_with(
        command,
        custom_instruction,
        PromptVars {
            language,
            app: None,
        },
    )
}

/// Like [`build_system_prompt`], with the full set of template variables.
/// Placeholders are expanded in the command prompt and in the one-off
/// instruction before anything else is appended.
pub fn build_system_prompt_with(
    command: &WritingCommand,
    custom_instruction: Option<&str>,
    vars: PromptVars<'_>,
) -> String {
    let language = vars.language;
    let custom_instruction = custom_instruction.map(|c| substitute_prompt_vars(c, vars));
    let custom_instruction = custom_instruction.as_deref();
    let mut system = substitute_prompt_vars(&command.prompt, vars);
    if command.kind == CommandKind::Replace {
        system.push('\n');
        system.push_str(REPLACE_OUTPUT_RULES);
    }
    if let Some(lang) = language.map(str::trim).filter(|l| !l.is_empty()) {
        system.push_str(&format!(
            "\nPreferred language: {lang}. Use it for the reply only when the instructions \
above do not specify an output language and the text is not clearly written in another \
language. An explicit language in the instructions always wins; otherwise keep the text's \
own language."
        ));
    }
    if let Some(extra) = custom_instruction {
        system.push_str(&format!("\nAdditional user instruction: {extra}"));
    }
    system
}

pub async fn run_command(
    provider: &dyn LlmProvider,
    command: &WritingCommand,
    input: &str,
    custom_instruction: Option<&str>,
    language: Option<&str>,
) -> Result<String, CoreError> {
    run_command_with(
        provider,
        command,
        input,
        custom_instruction,
        PromptVars {
            language,
            app: None,
        },
    )
    .await
}

/// [`run_command`] with the full set of prompt variables (`serve` passes the
/// source application name so `{{app}}` resolves).
pub async fn run_command_with(
    provider: &dyn LlmProvider,
    command: &WritingCommand,
    input: &str,
    custom_instruction: Option<&str>,
    vars: PromptVars<'_>,
) -> Result<String, CoreError> {
    if input.trim().is_empty() {
        return Err(CoreError::EmptyInput);
    }
    let system = build_system_prompt_with(command, custom_instruction, vars);

    let out = provider
        .complete(CompletionRequest {
            system,
            user: input.to_string(),
        })
        .await?;
    Ok(finish_output(command.kind, out, input))
}

/// [`run_command_with`] that streams the reply: `on_delta` receives each text
/// fragment as the model produces it, and the finished text is returned at
/// the end exactly as `run_command_with` would have returned it.
///
/// Fragments are forwarded raw. Only the returned text has
/// [`clean_replace_output`] applied for `Replace` commands, so a caller that
/// writes over the user's selection must wait for the return value instead
/// of assembling the fragments itself; fragments are for progress display.
pub async fn run_command_stream(
    provider: &dyn LlmProvider,
    command: &WritingCommand,
    input: &str,
    custom_instruction: Option<&str>,
    vars: PromptVars<'_>,
    on_delta: &mut DeltaSink<'_>,
) -> Result<String, CoreError> {
    if input.trim().is_empty() {
        return Err(CoreError::EmptyInput);
    }
    let system = build_system_prompt_with(command, custom_instruction, vars);

    let out = provider
        .complete_stream(
            CompletionRequest {
                system,
                user: input.to_string(),
            },
            on_delta,
        )
        .await?;
    Ok(finish_output(command.kind, out, input))
}

/// Post-process a finished reply for its command kind: `Replace` output is
/// cleaned of chat framing, `Popup` markdown is returned as-is.
fn finish_output(kind: CommandKind, out: String, input: &str) -> String {
    match kind {
        CommandKind::Replace => clean_replace_output(&out, input),
        CommandKind::Popup => out,
    }
}

/// Strip the chat framing models add around `Replace` output: an outer code
/// fence, a single leading "Here is the corrected text:" style line, and one
/// pair of wrapping quotes.
///
/// `input` is the original selection, and it decides what counts as framing:
/// a selection that is itself fenced, quoted, or opens with a "Here is ...:"
/// line round-trips unchanged through a compliant model, so removing that
/// layer would corrupt the user's own text. Each layer is therefore only
/// stripped when the selection did not already have it.
///
/// Internal whitespace and line breaks are preserved, and the result carries
/// the selection's own leading and trailing newlines, so a paragraph that was
/// selected with its terminating newline is pasted back with it.
pub fn clean_replace_output(raw: &str, input: &str) -> String {
    let mut text = raw;
    if unwrap_code_fence(input).is_none() {
        if let Some(inner) = unwrap_code_fence(text.trim_start_matches(['\n', '\r'])) {
            text = inner;
        }
    }
    if strip_preamble_line(input).is_none() {
        if let Some(rest) = strip_preamble_line(text.trim_start_matches(['\n', '\r'])) {
            text = rest;
        }
    }
    if strip_wrapping_quotes(input).is_none() {
        if let Some(inner) = strip_wrapping_quotes(text.trim_start_matches(['\n', '\r'])) {
            text = inner;
        }
    }
    let (lead, trail) = boundary_newlines(input);
    let body = text.trim_matches(['\n', '\r']);
    format!("{lead}{body}{trail}")
}

/// The runs of newline characters that open and close `text`. Blank input has
/// no boundaries to restore, so both are empty rather than the same run twice.
fn boundary_newlines(text: &str) -> (&str, &str) {
    if text.trim_matches(['\n', '\r']).is_empty() {
        return ("", "");
    }
    let lead_end = text.len() - text.trim_start_matches(['\n', '\r']).len();
    let trail_start = text.trim_end_matches(['\n', '\r']).len();
    (&text[..lead_end], &text[trail_start..])
}

/// If the whole text is a single fenced block (first line "```" plus an
/// optional language tag, last line "```"), return the lines in between.
fn unwrap_code_fence(text: &str) -> Option<&str> {
    let outer = text.trim_end();
    let (first, rest) = outer.split_once('\n')?;
    let tag = first.trim_end_matches('\r').strip_prefix("```")?;
    if !tag
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '#' | '.'))
    {
        return None;
    }
    let last_start = rest.rfind('\n').map_or(0, |i| i + 1);
    if rest[last_start..].trim_end_matches('\r') != "```" {
        return None;
    }
    Some(&rest[..last_start])
}

/// Remove one leading chat preamble line, i.e. a first line that opens with a
/// conversational marker and ends with a colon, when real content follows it.
fn strip_preamble_line(text: &str) -> Option<&str> {
    let (first, rest) = text.split_once('\n')?;
    if rest.trim().is_empty() {
        return None;
    }
    let line = first.trim().to_lowercase();
    if !line.ends_with(':') {
        return None;
    }
    const OPENERS: [&str; 10] = [
        "here's",
        "here\u{2019}s",
        "here is",
        "here are",
        "sure",
        "certainly",
        "of course",
        "okay",
        "ok",
        "alright",
    ];
    let opens_like_chat = OPENERS.iter().any(|opener| {
        line.strip_prefix(opener)
            .is_some_and(|after| !after.starts_with(|c: char| c.is_alphanumeric()))
    });
    opens_like_chat.then_some(rest)
}

/// Strip one pair of wrapping quotes (straight `"..."` or curly `“...”`) when
/// the text starts and ends with them and no other such quote appears inside.
fn strip_wrapping_quotes(text: &str) -> Option<&str> {
    let outer = text.trim_end();
    let open = outer.chars().next()?;
    let close = outer.chars().next_back()?;
    if outer.chars().count() < 2 {
        return None;
    }
    let inner = &outer[open.len_utf8()..outer.len() - close.len_utf8()];
    let clean = match (open, close) {
        ('"', '"') => !has_unescaped(inner, '"'),
        ('\u{201C}', '\u{201D}') => !inner.contains(['\u{201C}', '\u{201D}']),
        _ => return None,
    };
    clean.then_some(inner)
}

fn has_unescaped(text: &str, quote: char) -> bool {
    let mut escaped = false;
    for c in text.chars() {
        match c {
            '\\' => escaped = !escaped,
            c if c == quote && !escaped => return true,
            _ => escaped = false,
        }
    }
    false
}

pub fn find_command<'a>(
    commands: &'a [WritingCommand],
    id: &str,
) -> Result<&'a WritingCommand, CoreError> {
    commands
        .iter()
        .find(|c| c.id == id)
        .ok_or_else(|| CoreError::UnknownCommand(id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd() -> WritingCommand {
        WritingCommand {
            id: "proofread".into(),
            label: "Proofread".into(),
            kind: CommandKind::Replace,
            prompt: "Proofread the text.".into(),
            hotkey: None,
            model: None,
        }
    }

    fn popup_cmd() -> WritingCommand {
        WritingCommand {
            id: "summary".into(),
            label: "Summary".into(),
            kind: CommandKind::Popup,
            prompt: "Summarize the text.".into(),
            hotkey: None,
            model: None,
        }
    }

    #[test]
    fn popup_prompt_is_bare_without_extras() {
        assert_eq!(
            build_system_prompt(&popup_cmd(), None, None),
            "Summarize the text."
        );
    }

    #[test]
    fn replace_prompt_appends_output_rules() {
        let system = build_system_prompt(&cmd(), None, None);
        assert_eq!(
            system,
            format!("Proofread the text.\n{REPLACE_OUTPUT_RULES}")
        );
    }

    #[test]
    fn popup_prompt_omits_output_rules() {
        let system = build_system_prompt(&popup_cmd(), Some("Be brief."), Some("de"));
        assert!(!system.contains(REPLACE_OUTPUT_RULES));
    }

    #[test]
    fn prompt_includes_language_line() {
        let system = build_system_prompt(&cmd(), None, Some("es"));
        assert!(system.starts_with("Proofread the text.\n"));
        assert!(system.contains("Preferred language: es."));
    }

    #[test]
    fn language_hint_defers_to_command_instructions() {
        let system = build_system_prompt(&cmd(), None, Some("en"));
        assert!(system.contains("An explicit language in the instructions always wins"));
    }

    #[test]
    fn prompt_skips_blank_language() {
        assert_eq!(
            build_system_prompt(&popup_cmd(), None, Some("  ")),
            "Summarize the text."
        );
        assert_eq!(
            build_system_prompt(&cmd(), None, Some("  ")),
            build_system_prompt(&cmd(), None, None)
        );
    }

    #[test]
    fn prompt_orders_rules_then_language_then_instruction() {
        let system = build_system_prompt(&cmd(), Some("Keep it short."), Some("fr"));
        let prompt = system.find("Proofread the text.").unwrap();
        let rules = system.find(REPLACE_OUTPUT_RULES).unwrap();
        let lang = system.find("Preferred language: fr.").unwrap();
        let extra = system
            .find("Additional user instruction: Keep it short.")
            .unwrap();
        assert!(prompt < rules);
        assert!(rules < lang);
        assert!(lang < extra);
    }

    /// Existing-behaviour helper: a plain selection with no framing of its own.
    fn clean(raw: &str) -> String {
        clean_replace_output(raw, "The selection.")
    }

    #[test]
    fn clean_unwraps_fence_with_language_tag() {
        assert_eq!(clean("```text\nFixed sentence.\n```"), "Fixed sentence.");
        assert_eq!(
            clean("```markdown\n# Title\n\nBody.\n```\n"),
            "# Title\n\nBody."
        );
    }

    #[test]
    fn clean_unwraps_fence_without_tag() {
        assert_eq!(clean("```\nFixed sentence.\n```"), "Fixed sentence.");
    }

    #[test]
    fn clean_leaves_partial_fences_alone() {
        let opening_only = "```\nnot closed";
        assert_eq!(clean(opening_only), opening_only);
        let fence_in_prose = "Use ``` to open a block.";
        assert_eq!(clean(fence_in_prose), fence_in_prose);
    }

    #[test]
    fn clean_strips_here_is_preamble() {
        assert_eq!(
            clean("Here is the corrected text:\nI have two cats."),
            "I have two cats."
        );
        assert_eq!(
            clean("Here's the revised version:\n\nI have two cats."),
            "I have two cats."
        );
    }

    #[test]
    fn clean_strips_sure_preamble() {
        assert_eq!(
            clean("Sure! Here's a rewrite:\nWe should leave now."),
            "We should leave now."
        );
        assert_eq!(
            clean("Certainly, here is a friendlier take:\nHi there!"),
            "Hi there!"
        );
    }

    #[test]
    fn clean_only_strips_one_preamble_at_the_start() {
        let body = "Here is the plan:\nStep one.";
        assert_eq!(clean(&format!("Sure, here you go:\n{body}")), body);
        let mid = "Step one.\nHere is the plan:\nStep two.";
        assert_eq!(clean(mid), mid);
    }

    #[test]
    fn clean_keeps_colon_lines_that_are_content() {
        let list = "Ingredients:\n- eggs\n- milk";
        assert_eq!(clean(list), list);
        let lone = "Here is the text:";
        assert_eq!(clean(lone), lone);
    }

    #[test]
    fn clean_strips_straight_quote_wrapper() {
        assert_eq!(clean("\"I have two cats.\""), "I have two cats.");
    }

    #[test]
    fn clean_strips_curly_quote_wrapper() {
        assert_eq!(
            clean("\u{201C}I have two cats.\u{201D}"),
            "I have two cats."
        );
    }

    #[test]
    fn clean_keeps_quoted_phrase_inside_sentence() {
        let text = "She called it \"the best day ever\" and meant it.";
        assert_eq!(clean(text), text);
        let dialogue = "\"Stop,\" she said. \"Now.\"";
        assert_eq!(clean(dialogue), dialogue);
    }

    #[test]
    fn clean_keeps_text_that_only_starts_with_a_quote() {
        let text = "\"Quoted opener\" followed by prose.";
        assert_eq!(clean(text), text);
        let curly = "\u{201C}Quoted opener\u{201D} followed by prose.";
        assert_eq!(clean(curly), curly);
    }

    #[test]
    fn clean_keeps_internal_blank_lines() {
        let body = "First paragraph.\n\nSecond paragraph.\n\n  Indented third.";
        assert_eq!(clean(body), body);
        assert_eq!(clean(&format!("Here is the rewrite:\n{body}\n")), body);
        assert_eq!(clean(&format!("```\n{body}\n```")), body);
    }

    #[test]
    fn clean_returns_plain_text_identical() {
        let plain = "Nothing to see here, just a sentence.";
        assert_eq!(clean(plain), plain);
        let indented = "  leading spaces are content";
        assert_eq!(clean(indented), indented);
    }

    #[test]
    fn clean_applies_fence_then_preamble_then_quotes() {
        assert_eq!(
            clean("```\nHere is the corrected text:\n\"Two cats.\"\n```\n"),
            "Two cats."
        );
    }

    #[test]
    fn builtins_include_translate_using_language_placeholder() {
        let cmds = builtin_commands();
        let t = cmds
            .iter()
            .find(|c| c.id == "translate")
            .expect("translate builtin");
        assert_eq!(t.kind, CommandKind::Replace);
        assert!(t.prompt.contains("{{language}}"));
        let system = build_system_prompt(t, None, Some("es"));
        assert!(system.starts_with("Translate the text to es."), "{system}");
        assert!(
            !system.contains("{{"),
            "placeholder must be expanded: {system}"
        );
    }

    #[test]
    fn substitutes_language_and_app_with_whitespace_tolerance() {
        let vars = PromptVars {
            language: Some(" fr "),
            app: Some("Mail"),
        };
        assert_eq!(
            substitute_prompt_vars("Reply in {{language}} for {{ app }}.", vars),
            "Reply in fr for Mail."
        );
    }

    #[test]
    fn unknown_or_unterminated_placeholders_are_left_alone() {
        let vars = PromptVars::default();
        assert_eq!(
            substitute_prompt_vars("Keep {{selection}} and {{unknown}} and {{", vars),
            "Keep {{selection}} and {{unknown}} and {{"
        );
        assert_eq!(substitute_prompt_vars("no braces", vars), "no braces");
    }

    #[test]
    fn missing_values_fall_back_to_neutral_wording() {
        let vars = PromptVars {
            language: Some("  "),
            app: None,
        };
        assert_eq!(
            substitute_prompt_vars("Write in {{language}} as used in {{app}}.", vars),
            "Write in the same language as the text as used in the current application."
        );
    }

    #[test]
    fn instruction_placeholders_are_expanded_too() {
        let vars = PromptVars {
            language: Some("de"),
            app: Some("Slack"),
        };
        let system = build_system_prompt_with(&popup_cmd(), Some("Mention {{app}}."), vars);
        assert!(system.contains("Additional user instruction: Mention Slack."));
        assert!(system.contains("Preferred language: de."));
    }

    fn pack_cmd(id: &str, hotkey: Option<&str>) -> WritingCommand {
        WritingCommand {
            id: id.into(),
            label: id.to_uppercase(),
            kind: CommandKind::Replace,
            prompt: format!("Do {id}."),
            hotkey: hotkey.map(str::to_string),
            model: None,
        }
    }

    #[test]
    fn pack_round_trips_through_toml_and_reads_json() {
        let cmds = vec![pack_cmd("a", Some("ctrl+1")), pack_cmd("b", None)];
        let toml_text = render_command_pack(&cmds).unwrap();
        assert!(toml_text.contains("[[commands]]"), "{toml_text}");
        let back = parse_command_pack(&toml_text).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].hotkey.as_deref(), Some("ctrl+1"));

        let json_array = r#"[{"id":"x","label":"X","kind":"popup","prompt":"P"}]"#;
        assert_eq!(
            parse_command_pack(json_array).unwrap()[0].kind,
            CommandKind::Popup
        );
        let json_obj = r#"{"commands":[{"id":"y","label":"Y","kind":"replace","prompt":"Q"}]}"#;
        assert_eq!(parse_command_pack(json_obj).unwrap()[0].id, "y");

        // A whole config.toml is a valid pack too (only its commands are read).
        let cfg_like = "hotkey = \"ctrl+space\"\n[provider]\nkind = \"anthropic\"\nbase_url = \"\"\nmodel = \"m\"\n[[commands]]\nid = \"z\"\nlabel = \"Z\"\nkind = \"popup\"\nprompt = \"R\"\n";
        assert_eq!(parse_command_pack(cfg_like).unwrap()[0].id, "z");

        assert!(parse_command_pack("not a pack").is_err());
        assert!(parse_command_pack("hotkey = \"x\"").is_err(), "no commands");
    }

    #[test]
    fn merge_modes_and_hotkey_collisions() {
        let base = || vec![pack_cmd("a", Some("ctrl+1")), pack_cmd("b", None)];
        let incoming = || vec![pack_cmd("a", Some("ctrl+9")), pack_cmd("c", Some("ctrl+1"))];

        let mut e = base();
        let r = merge_commands(&mut e, incoming(), MergeMode::Skip, &[]);
        assert_eq!((r.added, r.replaced, r.skipped), (1, 0, 1));
        assert_eq!(e.len(), 3);
        assert_eq!(e[0].hotkey.as_deref(), Some("ctrl+1"), "existing untouched");
        assert_eq!(e[2].id, "c");
        assert_eq!(e[2].hotkey, None, "colliding hotkey dropped");
        assert_eq!(r.hotkeys_dropped, 1);

        let mut e = base();
        let r = merge_commands(&mut e, incoming(), MergeMode::Replace, &[]);
        assert_eq!((r.added, r.replaced, r.skipped), (1, 1, 0));
        assert_eq!(e[0].hotkey.as_deref(), Some("ctrl+9"), "replaced in place");
        assert_eq!(e[0].label, "A");
        // `c` wanted ctrl+1, which `a` no longer holds after the replace.
        assert_eq!(e[2].hotkey.as_deref(), Some("ctrl+1"));
        assert_eq!(r.hotkeys_dropped, 0);

        let mut e = base();
        e.push(pack_cmd("a-2", None));
        let r = merge_commands(&mut e, incoming(), MergeMode::KeepBoth, &[]);
        assert_eq!((r.added, r.replaced, r.skipped), (2, 0, 0));
        assert_eq!(r.renamed, vec![("a".to_string(), "a-3".to_string())]);
        let renamed = e.iter().find(|c| c.id == "a-3").unwrap();
        assert_eq!(renamed.label, "A (imported)");
        assert_eq!(renamed.hotkey.as_deref(), Some("ctrl+9"));
    }

    #[test]
    fn canonical_hotkey_folds_case_spacing_aliases_and_order() {
        let id = |s: &str| canonical_hotkey(s).unwrap();
        assert_eq!(id("ctrl+shift+p"), id(" Shift + Control + P "));
        assert_eq!(id("option+space"), id("ALT+Space"));
        assert_eq!(id("cmd+return"), id("meta+enter"));
        assert_eq!(id("ctrl+esc"), id("control+escape"));
        assert_ne!(id("ctrl+p"), id("ctrl+shift+p"));
        assert_ne!(id("ctrl+p"), id("cmd+p"));
        // No key token is what `parse_hotkey` rejects.
        assert_eq!(canonical_hotkey("ctrl+shift"), None);
        assert_eq!(canonical_hotkey("   "), None);
    }

    #[test]
    fn merge_compares_hotkey_identities_not_spellings() {
        let mut e = vec![pack_cmd("a", Some("ctrl+shift+p"))];
        let r = merge_commands(
            &mut e,
            vec![pack_cmd("c", Some(" Shift + Control + P "))],
            MergeMode::KeepBoth,
            &[],
        );
        assert_eq!(r.hotkeys_dropped, 1);
        assert_eq!(e[1].hotkey, None, "equivalent spelling still collides");
    }

    #[test]
    fn merge_reserves_picker_and_undo_hotkeys() {
        let mut e = vec![pack_cmd("a", None)];
        let r = merge_commands(
            &mut e,
            vec![
                pack_cmd("c", Some("CTRL + Shift + Space")),
                pack_cmd("d", Some("cmd+shift+z")),
            ],
            MergeMode::KeepBoth,
            &["ctrl+shift+space", "meta+shift+z"],
        );
        assert_eq!(r.hotkeys_dropped, 2);
        assert_eq!(e[1].hotkey, None, "picker hotkey reserved");
        assert_eq!(e[2].hotkey, None, "undo hotkey reserved");
    }

    #[test]
    fn skipped_commands_do_not_count_as_dropped_hotkeys() {
        let mut e = vec![pack_cmd("a", Some("ctrl+1"))];
        let r = merge_commands(
            &mut e,
            vec![pack_cmd("a", Some("ctrl+1"))],
            MergeMode::Skip,
            &[],
        );
        assert_eq!((r.added, r.replaced, r.skipped), (0, 0, 1));
        assert_eq!(r.hotkeys_dropped, 0, "nothing was kept, nothing dropped");
        assert_eq!(e[0].hotkey.as_deref(), Some("ctrl+1"));
    }

    struct EchoProvider;

    #[async_trait::async_trait]
    impl LlmProvider for EchoProvider {
        async fn complete(&self, req: CompletionRequest) -> Result<String, CoreError> {
            Ok(req.user)
        }
    }

    #[tokio::test]
    async fn empty_and_whitespace_input_are_rejected() {
        for input in ["", "   ", "\n\t"] {
            let err = run_command(&EchoProvider, &cmd(), input, None, None)
                .await
                .unwrap_err();
            assert!(
                matches!(err, CoreError::EmptyInput),
                "expected EmptyInput for {input:?}, got {err}"
            );
        }
        let out = run_command(&EchoProvider, &cmd(), "hello", None, None)
            .await
            .unwrap();
        assert_eq!(out, "hello");
    }

    struct FixedProvider(&'static str);

    #[async_trait::async_trait]
    impl LlmProvider for FixedProvider {
        async fn complete(&self, _req: CompletionRequest) -> Result<String, CoreError> {
            Ok(self.0.to_string())
        }
    }

    #[tokio::test]
    async fn run_command_cleans_replace_but_not_popup() {
        let fenced = "```\nHere is the corrected text:\nTwo cats.\n```";
        let provider = FixedProvider(fenced);
        let replaced = run_command(&provider, &cmd(), "Two cats!", None, None)
            .await
            .unwrap();
        assert_eq!(replaced, "Two cats.");
        let popup = run_command(&provider, &popup_cmd(), "Two cats!", None, None)
            .await
            .unwrap();
        assert_eq!(popup, fenced);
    }

    #[tokio::test]
    async fn run_command_keeps_framing_the_selection_already_had() {
        // A compliant provider returns the selection unchanged; none of it is
        // framing the cleaner may remove.
        for selection in [
            "```rust\nfn main() {}\n```",
            "\"I have two cats.\"",
            "Here is the plan:\nStep one.",
        ] {
            let out = run_command(&EchoProvider, &cmd(), selection, None, None)
                .await
                .unwrap();
            assert_eq!(out, selection, "selection round-tripped wrong");
        }
    }

    #[test]
    fn clean_keeps_a_fenced_selection_intact() {
        let selection = "```rust\nfn main() {}\n```";
        assert_eq!(clean_replace_output(selection, selection), selection);
        // A fence the model added around a plain selection is still stripped.
        assert_eq!(
            clean_replace_output("```\nplain text\n```", "plain text"),
            "plain text"
        );
    }

    #[test]
    fn clean_keeps_a_quoted_selection_intact() {
        let selection = "\"I have two cats.\"";
        assert_eq!(clean_replace_output(selection, selection), selection);
        let curly = "\u{201C}I have two cats.\u{201D}";
        assert_eq!(clean_replace_output(curly, curly), curly);
    }

    #[test]
    fn clean_keeps_a_selection_that_opens_with_a_colon_line() {
        let selection = "Here is the plan:\nStep one.";
        assert_eq!(clean_replace_output(selection, selection), selection);
    }

    #[test]
    fn clean_preserves_the_selections_trailing_newline() {
        assert_eq!(
            clean_replace_output("A paragraph.\n", "A paragraph.\n"),
            "A paragraph.\n"
        );
        // Also when the raw reply's newline is only a fence delimiter.
        assert_eq!(
            clean_replace_output("```\nA paragraph.\n```\n", "A paragraph.\n"),
            "A paragraph.\n"
        );
        // ...and restored when the model dropped it.
        assert_eq!(
            clean_replace_output("A paragraph.", "A paragraph.\n\n"),
            "A paragraph.\n\n"
        );
    }

    #[test]
    fn clean_preserves_the_selections_leading_newline() {
        assert_eq!(
            clean_replace_output("\nA paragraph.", "\nA paragraph."),
            "\nA paragraph."
        );
        // A selection without one does not gain one from the reply.
        assert_eq!(
            clean_replace_output("\n\nA paragraph.\n", "A paragraph."),
            "A paragraph."
        );
    }

    #[test]
    fn clean_keeps_leading_indentation_of_the_first_line() {
        assert_eq!(
            clean_replace_output("```\n    indented line\n```", "    indented line\n"),
            "    indented line\n"
        );
    }

    /// Streams the input back one line at a time (newline included), so
    /// tests can see exactly which fragments the sink received.
    struct LineStreamProvider;

    #[async_trait::async_trait]
    impl LlmProvider for LineStreamProvider {
        async fn complete(&self, req: CompletionRequest) -> Result<String, CoreError> {
            Ok(req.user)
        }

        async fn complete_stream(
            &self,
            req: CompletionRequest,
            on_delta: &mut DeltaSink<'_>,
        ) -> Result<String, CoreError> {
            for line in req.user.split_inclusive('\n') {
                on_delta(line);
            }
            Ok(req.user)
        }
    }

    #[tokio::test]
    async fn run_command_stream_cleans_replace_final_text_but_forwards_raw_deltas() {
        let fenced = "```\nHere is the corrected text:\nTwo cats.\n```";
        let mut seen: Vec<String> = Vec::new();
        let out = run_command_stream(
            &LineStreamProvider,
            &cmd(),
            fenced,
            None,
            PromptVars::default(),
            &mut |d: &str| seen.push(d.to_string()),
        )
        .await
        .unwrap();
        assert_eq!(out, "Two cats.", "final Replace text is cleaned");
        assert_eq!(
            seen,
            vec![
                "```\n",
                "Here is the corrected text:\n",
                "Two cats.\n",
                "```"
            ],
            "deltas are forwarded exactly as the provider sent them"
        );
        assert_eq!(seen.concat(), fenced);
    }

    #[tokio::test]
    async fn run_command_stream_leaves_popup_text_alone_and_rejects_empty_input() {
        let fenced = "```\nA summary.\n```";
        let mut seen: Vec<String> = Vec::new();
        let out = run_command_stream(
            &LineStreamProvider,
            &popup_cmd(),
            fenced,
            None,
            PromptVars::default(),
            &mut |d: &str| seen.push(d.to_string()),
        )
        .await
        .unwrap();
        assert_eq!(out, fenced);
        assert_eq!(seen.len(), 3);

        let mut calls = 0usize;
        let err = run_command_stream(
            &LineStreamProvider,
            &cmd(),
            "   ",
            None,
            PromptVars::default(),
            &mut |_: &str| calls += 1,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, CoreError::EmptyInput), "{err}");
        assert_eq!(calls, 0, "no provider call for empty input");
    }

    /// A provider with only `complete` still works through the streaming
    /// entry point: the whole reply arrives as one fragment.
    #[tokio::test]
    async fn run_command_stream_falls_back_to_one_fragment() {
        let mut seen: Vec<String> = Vec::new();
        let out = run_command_stream(
            &EchoProvider,
            &popup_cmd(),
            "hello",
            None,
            PromptVars::default(),
            &mut |d: &str| seen.push(d.to_string()),
        )
        .await
        .unwrap();
        assert_eq!(out, "hello");
        assert_eq!(seen, vec!["hello"]);
    }
}
