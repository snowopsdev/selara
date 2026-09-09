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
    /// Apps this command is offered in. Empty (the default) means every app.
    /// Entries match the frontmost app's name or bundle id, case-insensitively
    /// and whitespace-trimmed, with a trailing `*` acting as a prefix glob —
    /// the same matching style as `excluded_apps`. See [`command_applies_to`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub apps: Vec<String>,
}

/// True when `cmd` should be offered in the app a selection came from.
///
/// An empty `apps` list (or one holding only blank entries) means the command
/// is universal. Otherwise an entry matches when it equals the app name or the
/// bundle id, compared trimmed and lowercased; a trailing `*` matches by
/// prefix, and a bare `*` matches every app. A command that names apps but is
/// checked against an unknown app (no name and no bundle id) does not apply —
/// an allow-list only allows what it can identify.
pub fn command_applies_to(
    cmd: &WritingCommand,
    app_name: Option<&str>,
    bundle_id: Option<&str>,
) -> bool {
    let entries: Vec<String> = cmd
        .apps
        .iter()
        .map(|a| a.trim().to_lowercase())
        .filter(|a| !a.is_empty())
        .collect();
    if entries.is_empty() {
        return true;
    }
    let name = app_name
        .map(|n| n.trim().to_lowercase())
        .filter(|n| !n.is_empty());
    let bundle = bundle_id
        .map(|b| b.trim().to_lowercase())
        .filter(|b| !b.is_empty());
    let candidates = [name.as_deref(), bundle.as_deref()];
    entries.iter().any(|entry| {
        if entry == "*" {
            return true;
        }
        match entry.strip_suffix('*') {
            Some(prefix) => candidates.iter().flatten().any(|c| c.starts_with(prefix)),
            None => candidates.iter().flatten().any(|c| *c == entry),
        }
    })
}

/// The commands from `cmds` that apply to this app, in their original order.
pub fn commands_for_app<'a>(
    cmds: &'a [WritingCommand],
    app_name: Option<&str>,
    bundle_id: Option<&str>,
) -> Vec<&'a WritingCommand> {
    cmds.iter()
        .filter(|c| command_applies_to(c, app_name, bundle_id))
        .collect()
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
            apps: Vec::new(),
        },
        WritingCommand {
            id: "rewrite".into(),
            label: "Rewrite".into(),
            kind: CommandKind::Replace,
            prompt: "Rewrite the text for clarity and flow. Keep the original meaning. Return only the rewritten text.".into(),
            hotkey: None,
            model: None,
            apps: Vec::new(),
        },
        WritingCommand {
            id: "friendly".into(),
            label: "Friendly".into(),
            kind: CommandKind::Replace,
            prompt: "Rewrite the text in a warm, friendly tone. Return only the rewritten text.".into(),
            hotkey: None,
            model: None,
            apps: Vec::new(),
        },
        WritingCommand {
            id: "professional".into(),
            label: "Professional".into(),
            kind: CommandKind::Replace,
            prompt: "Rewrite the text in a clear, professional tone. Return only the rewritten text.".into(),
            hotkey: None,
            model: None,
            apps: Vec::new(),
        },
        WritingCommand {
            id: "concise".into(),
            label: "Concise".into(),
            kind: CommandKind::Replace,
            prompt: "Make the text more concise without losing key meaning. Return only the rewritten text.".into(),
            hotkey: None,
            model: None,
            apps: Vec::new(),
        },
        WritingCommand {
            id: "summary".into(),
            label: "Summary".into(),
            kind: CommandKind::Popup,
            prompt: "Summarize the text clearly in markdown. Use short paragraphs or bullets as needed.".into(),
            hotkey: None,
            model: None,
            apps: Vec::new(),
        },
        WritingCommand {
            id: "key_points".into(),
            label: "Key Points".into(),
            kind: CommandKind::Popup,
            prompt: "Extract the key points as a markdown bullet list.".into(),
            hotkey: None,
            model: None,
            apps: Vec::new(),
        },
        WritingCommand {
            id: "table".into(),
            label: "Table".into(),
            kind: CommandKind::Popup,
            prompt: "Convert the useful information in the text into a markdown table.".into(),
            hotkey: None,
            model: None,
            apps: Vec::new(),
        },
        WritingCommand {
            id: "translate".into(),
            label: "Translate".into(),
            kind: CommandKind::Replace,
            prompt: "Translate the text to {{language}}. Return only the translation.".into(),
            hotkey: None,
            model: None,
            apps: Vec::new(),
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

/// Merge `incoming` into `existing` according to `mode`. Hotkeys that would
/// collide with a command already in the list are dropped, never duplicated,
/// so a reload of `serve` cannot fail on a duplicate binding.
pub fn merge_commands(
    existing: &mut Vec<WritingCommand>,
    incoming: Vec<WritingCommand>,
    mode: MergeMode,
) -> MergeReport {
    let mut report = MergeReport::default();
    for mut cmd in incoming {
        let pos = existing.iter().position(|c| c.id == cmd.id);
        // Hotkey collisions: compare against everything except the command
        // this one is about to replace.
        if let Some(hk) = cmd
            .hotkey
            .as_deref()
            .map(str::trim)
            .filter(|h| !h.is_empty())
        {
            let taken = existing.iter().enumerate().any(|(i, c)| {
                Some(i) != pos.filter(|_| mode == MergeMode::Replace)
                    && c.hotkey.as_deref().map(str::trim) == Some(hk)
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
            (Some(_), MergeMode::Skip) => report.skipped += 1,
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
    Ok(finish_output(command.kind, out))
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
    Ok(finish_output(command.kind, out))
}

/// Post-process a finished reply for its command kind: `Replace` output is
/// cleaned of chat framing, `Popup` markdown is returned as-is.
fn finish_output(kind: CommandKind, out: String) -> String {
    match kind {
        CommandKind::Replace => clean_replace_output(&out),
        CommandKind::Popup => out,
    }
}

/// Strip the chat framing models add around `Replace` output: an outer code
/// fence, a single leading "Here is the corrected text:" style line, and one
/// pair of wrapping quotes. Internal whitespace and line breaks are preserved;
/// only leading and trailing newlines are trimmed at the end.
pub fn clean_replace_output(raw: &str) -> String {
    let mut text = raw.trim_start_matches(['\n', '\r']);
    if let Some(inner) = unwrap_code_fence(text) {
        text = inner;
    }
    if let Some(rest) = strip_preamble_line(text) {
        text = rest;
    }
    if let Some(inner) = strip_wrapping_quotes(text) {
        text = inner;
    }
    text.trim_matches(['\n', '\r']).to_string()
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
            apps: Vec::new(),
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
            apps: Vec::new(),
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

    #[test]
    fn clean_unwraps_fence_with_language_tag() {
        assert_eq!(
            clean_replace_output("```text\nFixed sentence.\n```"),
            "Fixed sentence."
        );
        assert_eq!(
            clean_replace_output("```markdown\n# Title\n\nBody.\n```\n"),
            "# Title\n\nBody."
        );
    }

    #[test]
    fn clean_unwraps_fence_without_tag() {
        assert_eq!(
            clean_replace_output("```\nFixed sentence.\n```"),
            "Fixed sentence."
        );
    }

    #[test]
    fn clean_leaves_partial_fences_alone() {
        let opening_only = "```\nnot closed";
        assert_eq!(clean_replace_output(opening_only), opening_only);
        let fence_in_prose = "Use ``` to open a block.";
        assert_eq!(clean_replace_output(fence_in_prose), fence_in_prose);
    }

    #[test]
    fn clean_strips_here_is_preamble() {
        assert_eq!(
            clean_replace_output("Here is the corrected text:\nI have two cats."),
            "I have two cats."
        );
        assert_eq!(
            clean_replace_output("Here's the revised version:\n\nI have two cats."),
            "I have two cats."
        );
    }

    #[test]
    fn clean_strips_sure_preamble() {
        assert_eq!(
            clean_replace_output("Sure! Here's a rewrite:\nWe should leave now."),
            "We should leave now."
        );
        assert_eq!(
            clean_replace_output("Certainly, here is a friendlier take:\nHi there!"),
            "Hi there!"
        );
    }

    #[test]
    fn clean_only_strips_one_preamble_at_the_start() {
        let body = "Here is the plan:\nStep one.";
        assert_eq!(
            clean_replace_output(&format!("Sure, here you go:\n{body}")),
            body
        );
        let mid = "Step one.\nHere is the plan:\nStep two.";
        assert_eq!(clean_replace_output(mid), mid);
    }

    #[test]
    fn clean_keeps_colon_lines_that_are_content() {
        let list = "Ingredients:\n- eggs\n- milk";
        assert_eq!(clean_replace_output(list), list);
        let lone = "Here is the text:";
        assert_eq!(clean_replace_output(lone), lone);
    }

    #[test]
    fn clean_strips_straight_quote_wrapper() {
        assert_eq!(
            clean_replace_output("\"I have two cats.\""),
            "I have two cats."
        );
    }

    #[test]
    fn clean_strips_curly_quote_wrapper() {
        assert_eq!(
            clean_replace_output("\u{201C}I have two cats.\u{201D}"),
            "I have two cats."
        );
    }

    #[test]
    fn clean_keeps_quoted_phrase_inside_sentence() {
        let text = "She called it \"the best day ever\" and meant it.";
        assert_eq!(clean_replace_output(text), text);
        let dialogue = "\"Stop,\" she said. \"Now.\"";
        assert_eq!(clean_replace_output(dialogue), dialogue);
    }

    #[test]
    fn clean_keeps_text_that_only_starts_with_a_quote() {
        let text = "\"Quoted opener\" followed by prose.";
        assert_eq!(clean_replace_output(text), text);
        let curly = "\u{201C}Quoted opener\u{201D} followed by prose.";
        assert_eq!(clean_replace_output(curly), curly);
    }

    #[test]
    fn clean_keeps_internal_blank_lines() {
        let body = "First paragraph.\n\nSecond paragraph.\n\n  Indented third.";
        assert_eq!(clean_replace_output(body), body);
        assert_eq!(
            clean_replace_output(&format!("Here is the rewrite:\n{body}\n")),
            body
        );
        assert_eq!(clean_replace_output(&format!("```\n{body}\n```")), body);
    }

    #[test]
    fn clean_returns_plain_text_identical() {
        let plain = "Nothing to see here, just a sentence.";
        assert_eq!(clean_replace_output(plain), plain);
        let indented = "  leading spaces are content";
        assert_eq!(clean_replace_output(indented), indented);
    }

    #[test]
    fn clean_applies_fence_then_preamble_then_quotes() {
        assert_eq!(
            clean_replace_output("```\nHere is the corrected text:\n\"Two cats.\"\n```\n"),
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

    fn app_cmd(id: &str, apps: &[&str]) -> WritingCommand {
        WritingCommand {
            apps: apps.iter().map(|a| (*a).to_string()).collect(),
            ..cmd_with_id(id)
        }
    }

    fn cmd_with_id(id: &str) -> WritingCommand {
        WritingCommand {
            id: id.into(),
            label: id.to_uppercase(),
            kind: CommandKind::Replace,
            prompt: format!("Do {id}."),
            hotkey: None,
            model: None,
            apps: Vec::new(),
        }
    }

    #[test]
    fn a_command_without_apps_applies_everywhere() {
        let c = cmd_with_id("proofread");
        assert!(command_applies_to(&c, Some("Mail"), Some("com.apple.mail")));
        assert!(command_applies_to(&c, None, None));
    }

    #[test]
    fn blank_app_entries_are_ignored_and_leave_the_command_universal() {
        let c = app_cmd("proofread", &["  ", ""]);
        assert!(command_applies_to(&c, Some("Xcode"), None));
    }

    #[test]
    fn a_command_with_apps_does_not_apply_to_another_app() {
        let c = app_cmd("reply", &["Mail"]);
        assert!(command_applies_to(&c, Some("Mail"), Some("com.apple.mail")));
        assert!(!command_applies_to(
            &c,
            Some("Slack"),
            Some("com.tinyspeck.slackmacgap")
        ));
        assert!(!command_applies_to(&c, Some("Notes"), None));
    }

    #[test]
    fn app_entries_match_names_and_bundle_ids_case_insensitively() {
        let by_name = app_cmd("reply", &["  mAiL  "]);
        assert!(command_applies_to(&by_name, Some("Mail"), None));
        let by_bundle = app_cmd("post", &["COM.TinySpeck.SlackMacGap"]);
        assert!(command_applies_to(
            &by_bundle,
            Some("Slack"),
            Some("com.tinyspeck.slackmacgap")
        ));
        // The name alone does not satisfy an entry written as a bundle id.
        assert!(!command_applies_to(&by_bundle, Some("Slack"), None));
    }

    #[test]
    fn a_trailing_star_matches_by_prefix_and_a_bare_star_matches_everything() {
        let prefixed = app_cmd("code", &["Xcode*"]);
        assert!(command_applies_to(&prefixed, Some("Xcode-beta"), None));
        assert!(command_applies_to(&prefixed, Some("Xcode"), None));
        assert!(!command_applies_to(&prefixed, Some("Terminal"), None));

        let bundle_prefix = app_cmd("note", &["com.apple.*"]);
        assert!(command_applies_to(
            &bundle_prefix,
            Some("Notes"),
            Some("com.apple.notes")
        ));
        assert!(!command_applies_to(
            &bundle_prefix,
            Some("Slack"),
            Some("com.tinyspeck.slackmacgap")
        ));

        let everywhere = app_cmd("any", &["*"]);
        assert!(command_applies_to(&everywhere, Some("Anything"), None));
    }

    #[test]
    fn a_restricted_command_does_not_apply_to_an_unidentified_app() {
        let c = app_cmd("reply", &["Mail"]);
        assert!(!command_applies_to(&c, None, None));
        assert!(!command_applies_to(&c, Some("   "), Some("")));
    }

    #[test]
    fn commands_for_app_keeps_order_and_drops_the_ones_that_do_not_apply() {
        let all = vec![
            cmd_with_id("proofread"),
            app_cmd("reply", &["Mail"]),
            app_cmd("standup", &["com.tinyspeck.slackmacgap"]),
            cmd_with_id("summary"),
        ];
        let ids: Vec<&str> = commands_for_app(&all, Some("Mail"), Some("com.apple.mail"))
            .iter()
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(ids, vec!["proofread", "reply", "summary"]);

        let slack: Vec<&str> =
            commands_for_app(&all, Some("Slack"), Some("com.tinyspeck.slackmacgap"))
                .iter()
                .map(|c| c.id.as_str())
                .collect();
        assert_eq!(slack, vec!["proofread", "standup", "summary"]);
    }

    #[test]
    fn apps_round_trip_through_a_command_pack_and_stay_off_the_wire_when_empty() {
        let cmds = vec![app_cmd("reply", &["Mail", "com.apple.notes"])];
        let toml_text = render_command_pack(&cmds).unwrap();
        assert!(toml_text.contains("apps"), "{toml_text}");
        let back = parse_command_pack(&toml_text).unwrap();
        assert_eq!(back[0].apps, vec!["Mail", "com.apple.notes"]);

        let plain = render_command_pack(&[cmd_with_id("proofread")]).unwrap();
        assert!(!plain.contains("apps"), "{plain}");
        assert!(parse_command_pack(&plain).unwrap()[0].apps.is_empty());
    }

    fn pack_cmd(id: &str, hotkey: Option<&str>) -> WritingCommand {
        WritingCommand {
            id: id.into(),
            label: id.to_uppercase(),
            kind: CommandKind::Replace,
            prompt: format!("Do {id}."),
            hotkey: hotkey.map(str::to_string),
            model: None,
            apps: Vec::new(),
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
        let r = merge_commands(&mut e, incoming(), MergeMode::Skip);
        assert_eq!((r.added, r.replaced, r.skipped), (1, 0, 1));
        assert_eq!(e.len(), 3);
        assert_eq!(e[0].hotkey.as_deref(), Some("ctrl+1"), "existing untouched");
        assert_eq!(e[2].id, "c");
        assert_eq!(e[2].hotkey, None, "colliding hotkey dropped");
        assert_eq!(r.hotkeys_dropped, 1);

        let mut e = base();
        let r = merge_commands(&mut e, incoming(), MergeMode::Replace);
        assert_eq!((r.added, r.replaced, r.skipped), (1, 1, 0));
        assert_eq!(e[0].hotkey.as_deref(), Some("ctrl+9"), "replaced in place");
        assert_eq!(e[0].label, "A");
        // `c` wanted ctrl+1, which `a` no longer holds after the replace.
        assert_eq!(e[2].hotkey.as_deref(), Some("ctrl+1"));
        assert_eq!(r.hotkeys_dropped, 0);

        let mut e = base();
        e.push(pack_cmd("a-2", None));
        let r = merge_commands(&mut e, incoming(), MergeMode::KeepBoth);
        assert_eq!((r.added, r.replaced, r.skipped), (2, 0, 0));
        assert_eq!(r.renamed, vec![("a".to_string(), "a-3".to_string())]);
        let renamed = e.iter().find(|c| c.id == "a-3").unwrap();
        assert_eq!(renamed.label, "A (imported)");
        assert_eq!(renamed.hotkey.as_deref(), Some("ctrl+9"));
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

    #[tokio::test]
    async fn run_command_cleans_replace_but_not_popup() {
        let fenced = "```\nHere is the corrected text:\nTwo cats.\n```";
        let replaced = run_command(&EchoProvider, &cmd(), fenced, None, None)
            .await
            .unwrap();
        assert_eq!(replaced, "Two cats.");
        let popup = run_command(&EchoProvider, &popup_cmd(), fenced, None, None)
            .await
            .unwrap();
        assert_eq!(popup, fenced);
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
