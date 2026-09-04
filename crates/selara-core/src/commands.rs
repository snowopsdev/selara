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

pub async fn run_command(
    provider: &dyn LlmProvider,
    command: &WritingCommand,
    input: &str,
    custom_instruction: Option<&str>,
) -> Result<String, CoreError> {
    let system = if let Some(extra) = custom_instruction {
        format!("{}\nAdditional user instruction: {}", command.prompt, extra)
    } else {
        command.prompt.clone()
    };

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
