use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::providers::{CompletionRequest, LlmProvider};

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
}

pub fn builtin_commands() -> Vec<WritingCommand> {
    vec![
        WritingCommand {
            id: "proofread".into(),
            label: "Proofread".into(),
            kind: CommandKind::Replace,
            prompt: "Proofread the text. Fix grammar, spelling, and punctuation only. Keep meaning and voice. Return only the corrected text.".into(),
            hotkey: None,
        },
        WritingCommand {
            id: "rewrite".into(),
            label: "Rewrite".into(),
            kind: CommandKind::Replace,
            prompt: "Rewrite the text for clarity and flow. Keep the original meaning. Return only the rewritten text.".into(),
            hotkey: None,
        },
        WritingCommand {
            id: "friendly".into(),
            label: "Friendly".into(),
            kind: CommandKind::Replace,
            prompt: "Rewrite the text in a warm, friendly tone. Return only the rewritten text.".into(),
            hotkey: None,
        },
        WritingCommand {
            id: "professional".into(),
            label: "Professional".into(),
            kind: CommandKind::Replace,
            prompt: "Rewrite the text in a clear, professional tone. Return only the rewritten text.".into(),
            hotkey: None,
        },
        WritingCommand {
            id: "concise".into(),
            label: "Concise".into(),
            kind: CommandKind::Replace,
            prompt: "Make the text more concise without losing key meaning. Return only the rewritten text.".into(),
            hotkey: None,
        },
        WritingCommand {
            id: "summary".into(),
            label: "Summary".into(),
            kind: CommandKind::Popup,
            prompt: "Summarize the text clearly in markdown. Use short paragraphs or bullets as needed.".into(),
            hotkey: None,
        },
        WritingCommand {
            id: "key_points".into(),
            label: "Key Points".into(),
            kind: CommandKind::Popup,
            prompt: "Extract the key points as a markdown bullet list.".into(),
            hotkey: None,
        },
        WritingCommand {
            id: "table".into(),
            label: "Table".into(),
            kind: CommandKind::Popup,
            prompt: "Convert the useful information in the text into a markdown table.".into(),
            hotkey: None,
        },
    ]
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
    let mut system = command.prompt.clone();
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
    if input.trim().is_empty() {
        return Err(CoreError::EmptyInput);
    }
    let system = build_system_prompt(command, custom_instruction, language);

    let out = provider
        .complete(CompletionRequest {
            system,
            user: input.to_string(),
        })
        .await?;
    Ok(match command.kind {
        CommandKind::Replace => clean_replace_output(&out),
        CommandKind::Popup => out,
    })
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
        }
    }

    fn popup_cmd() -> WritingCommand {
        WritingCommand {
            id: "summary".into(),
            label: "Summary".into(),
            kind: CommandKind::Popup,
            prompt: "Summarize the text.".into(),
            hotkey: None,
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
}
