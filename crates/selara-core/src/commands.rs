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

/// Assemble the system prompt: the command's prompt, then the preferred
/// language from config (blank means no hint), then any one-off instruction.
pub fn build_system_prompt(
    command: &WritingCommand,
    custom_instruction: Option<&str>,
    language: Option<&str>,
) -> String {
    let mut system = command.prompt.clone();
    if let Some(lang) = language.map(str::trim).filter(|l| !l.is_empty()) {
        system.push_str(&format!(
            "\nPreferred language: {lang}. Reply in this language unless the text is clearly \
written in another language; in that case keep the text's language."
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
    let system = build_system_prompt(command, custom_instruction, language);

    provider
        .complete(CompletionRequest {
            system,
            user: input.to_string(),
        })
        .await
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

    #[test]
    fn prompt_is_bare_without_extras() {
        assert_eq!(
            build_system_prompt(&cmd(), None, None),
            "Proofread the text."
        );
    }

    #[test]
    fn prompt_includes_language_line() {
        let system = build_system_prompt(&cmd(), None, Some("es"));
        assert!(system.starts_with("Proofread the text.\n"));
        assert!(system.contains("Preferred language: es."));
    }

    #[test]
    fn prompt_skips_blank_language() {
        assert_eq!(
            build_system_prompt(&cmd(), None, Some("  ")),
            "Proofread the text."
        );
    }

    #[test]
    fn prompt_puts_instruction_after_language() {
        let system = build_system_prompt(&cmd(), Some("Keep it short."), Some("fr"));
        let lang = system.find("Preferred language: fr.").unwrap();
        let extra = system
            .find("Additional user instruction: Keep it short.")
            .unwrap();
        assert!(lang < extra);
    }
}
