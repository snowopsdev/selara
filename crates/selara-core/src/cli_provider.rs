//! Buffered writing adapters for locally installed CLIs. Text goes over stdin,
//! never a shell or process arguments. Runtime availability is separate from auth.
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};

use crate::error::CoreError;
use crate::providers::{CompletionRequest, LlmProvider, ProviderKind};
use crate::usage::{self, TokenUsage};

const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
const RUN_TIMEOUT: Duration = Duration::from_secs(180);
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);

fn label(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::ClaudeCli => "Claude Code",
        ProviderKind::CursorCli => "Cursor",
        ProviderKind::OpenCodeCli => "OpenCode",
        _ => "CLI",
    }
}
fn failure(message: impl Into<String>) -> CoreError {
    CoreError::Provider(message.into())
}

#[derive(Debug, Serialize)]
pub struct CliStatus {
    pub installed: bool,
    pub version: Option<String>,
    pub binary: Option<String>,
    pub message: String,
}

fn executable(path: &Path) -> bool {
    let Ok(meta) = path.metadata() else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}
fn resolve_binary(kind: ProviderKind, configured: Option<&Path>) -> Result<PathBuf, CoreError> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| {
            std::env::split_paths(&p)
                .filter(|d| d.is_absolute())
                .collect()
        })
        .unwrap_or_default();
    // Finder-launched macOS apps do not inherit a login shell's PATH.
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(&home).join(".local/bin"));
        dirs.push(PathBuf::from(home).join(".opencode/bin"));
    }
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"].map(PathBuf::from));
    resolve_binary_in(kind, configured, &dirs)
}

fn resolve_binary_in(
    kind: ProviderKind,
    configured: Option<&Path>,
    dirs: &[PathBuf],
) -> Result<PathBuf, CoreError> {
    if !kind.is_cli() {
        return Err(failure("Not an external CLI provider."));
    }
    let defaults: &[&str] = match kind {
        ProviderKind::ClaudeCli => &["claude"],
        // Generic `agent` may belong to another product (for example, Grok).
        ProviderKind::CursorCli => &["cursor-agent"],
        ProviderKind::OpenCodeCli => &["opencode"],
        _ => unreachable!(),
    };
    let configured = configured.filter(|p| !p.as_os_str().is_empty());
    if let Some(path) = configured {
        if path.is_absolute() {
            return if executable(path) {
                Ok(path.to_path_buf())
            } else {
                Err(failure(format!("{} executable was not found or is not executable. Check its path in Providers.", label(kind))))
            };
        }
        if path.components().count() != 1 || path == Path::new(".") || path == Path::new("..") {
            return Err(failure(
                "CLI executable must be an absolute path or a command name.",
            ));
        }
    }
    let names: Vec<PathBuf> = configured
        .map(|p| vec![p.to_path_buf()])
        .unwrap_or_else(|| defaults.iter().map(PathBuf::from).collect());
    for name in names {
        for dir in dirs {
            let candidate = dir.join(&name);
            if executable(&candidate) {
                return Ok(candidate);
            }
        }
    }
    Err(failure(format!("{} is not installed or could not be found. Install its CLI, or set the executable path in Providers.", label(kind))))
}

// Also kill subprocesses on cancellation: a shell wrapper or CLI helper must
// not outlive a discarded completion future and keep generating in background.
struct RunningChild {
    child: Child,
    #[cfg(unix)]
    group: Option<u32>,
}
impl Drop for RunningChild {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.group.take() {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        let _ = self.child.start_kill();
    }
}
async fn bounded(reader: impl AsyncRead + Unpin) -> Result<Vec<u8>, CoreError> {
    let mut out = Vec::new();
    reader
        .take((OUTPUT_LIMIT + 1) as u64)
        .read_to_end(&mut out)
        .await?;
    if out.len() > OUTPUT_LIMIT {
        return Err(failure(
            "CLI output exceeded the size limit; no text was replaced.",
        ));
    }
    Ok(out)
}
async fn run_process(
    mut cmd: Command,
    input: &[u8],
    timeout: Duration,
) -> Result<Vec<u8>, CoreError> {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    let child = cmd.spawn().map_err(|_| failure("Could not start the provider CLI. Check the executable path and its runtime dependencies."))?;
    #[cfg(unix)]
    let group = child.id();
    let mut running = RunningChild {
        child,
        #[cfg(unix)]
        group,
    };
    let mut stdin = running.child.stdin.take().expect("piped stdin");
    let stdout = running.child.stdout.take().expect("piped stdout");
    let stderr = running.child.stderr.take().expect("piped stderr");
    let work = async {
        let write = async {
            // A CLI can reject its flags before reading stdin. Let its exit
            // status describe that failure instead of surfacing a broken pipe.
            let result = stdin.write_all(input).await;
            drop(stdin);
            match result {
                Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => Err(e.into()),
                _ => Ok(()),
            }
        };
        let wait = async { running.child.wait().await.map_err(CoreError::from) };
        let (_, out, _, status) = tokio::try_join!(write, bounded(stdout), bounded(stderr), wait)?;
        if !status.success() {
            return Err(failure(format!("Provider CLI exited unsuccessfully ({}). Check its sign-in, model access, and installed version in Providers.", status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into()))));
        }
        Ok(out)
    };
    tokio::time::timeout(timeout, work)
        .await
        .map_err(|_| failure("Provider CLI timed out; no text was replaced."))?
}

pub async fn inspect(kind: ProviderKind, binary: Option<PathBuf>) -> Result<CliStatus, CoreError> {
    let path = match resolve_binary(kind, binary.as_deref()) {
        Ok(path) => path,
        Err(e) => {
            return Ok(CliStatus {
                installed: false,
                version: None,
                binary: None,
                message: e.to_string(),
            })
        }
    };
    let dir = tempfile::Builder::new()
        .prefix("selara-cli-check-")
        .tempdir()?;
    let mut cmd = Command::new(&path);
    cmd.arg("--version").current_dir(dir.path());
    let out = run_process(cmd, &[], VERSION_TIMEOUT).await?;
    let text =
        String::from_utf8(out).map_err(|_| failure("CLI version output was not valid text."))?;
    let version = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim();
    if version.is_empty() || version.len() > 120 || version.chars().any(char::is_control) {
        return Err(failure("CLI returned an unrecognized version response."));
    }
    Ok(CliStatus {
        installed: true,
        version: Some(version.into()),
        binary: Some(path.to_string_lossy().into()),
        message: format!(
            "{} is installed. Account and model access are checked when a writing command runs.",
            label(kind)
        ),
    })
}

pub struct CliProvider {
    kind: ProviderKind,
    model: String,
    binary: Option<PathBuf>,
}
impl CliProvider {
    pub fn new(kind: ProviderKind, model: String, binary: Option<PathBuf>) -> Self {
        Self {
            kind,
            model,
            binary,
        }
    }

    fn command(&self, binary: &Path, dir: &Path) -> Result<Command, CoreError> {
        let mut cmd = Command::new(binary);
        cmd.current_dir(dir).env("PWD", dir).env("NO_COLOR", "1");
        match self.kind {
            ProviderKind::ClaudeCli => {
                // safe-mode suppresses hooks, skills, plugins and MCP while
                // retaining the user's auth. Explicit tool denial is separate.
                cmd.args([
                    "--print",
                    "--output-format",
                    "json",
                    "--safe-mode",
                    "--tools",
                    "",
                    "--disallowedTools",
                    "*",
                    "--permission-mode",
                    "dontAsk",
                    "--permission-prompts",
                    "none",
                    "--no-session-persistence",
                    "--disable-slash-commands",
                ]);
            }
            ProviderKind::CursorCli => {
                let cfg = dir.join("cursor-config");
                std::fs::create_dir(&cfg)?;
                std::fs::write(cfg.join("cli-config.json"), serde_json::to_vec(&json!({
                    "version": 1, "editor": {"vimMode": false}, "approvalMode": "allowlist",
                    "permissions": {"allow": [], "deny": ["Shell(*)", "Read(**)", "Read(/**)", "Write(**)", "Write(/**)", "WebFetch(*)", "Mcp(*:*)"]}
                })).map_err(|_| failure("Could not prepare CLI permissions."))?)?;
                // Override CLI permissions without copying credentials. Cursor
                // still runs local/team hooks from its normal account settings;
                // this behavior is disclosed in the provider settings.
                cmd.env("CURSOR_CONFIG_DIR", cfg)
                    .args([
                        "--print",
                        "--output-format",
                        "json",
                        "--mode",
                        "ask",
                        "--sandbox",
                        "enabled",
                        // Trust only this freshly created private workspace.
                        // This does not bypass the tool permission deny list.
                        "--trust",
                        "--workspace",
                    ])
                    .arg(dir);
            }
            ProviderKind::OpenCodeCli => {
                let cfg = dir.join("config");
                std::fs::create_dir(&cfg)?;
                // Auth remains in OpenCode's data store; isolate global/project
                // customizations and plugins from the selected text.
                cmd.env("XDG_CONFIG_HOME", &cfg)
                    .env("OPENCODE_CONFIG_DIR", &cfg)
                    .env_remove("OPENCODE_CONFIG")
                    .env(
                        "OPENCODE_CONFIG_CONTENT",
                        r#"{"permission":{"*":"deny"},"share":"disabled"}"#,
                    )
                    .env("OPENCODE_PERMISSION", r#"{"*":"deny"}"#)
                    // Built-in plugins include OAuth adapters. --pure disables
                    // external plugins without removing those auth providers.
                    .env_remove("OPENCODE_DISABLE_DEFAULT_PLUGINS")
                    .env("OPENCODE_DISABLE_CLAUDE_CODE", "true")
                    .env("OPENCODE_DISABLE_EXTERNAL_SKILLS", "true")
                    .args(["run", "--format", "json", "--pure", "--dir"])
                    .arg(dir);
            }
            _ => return Err(failure("Not an external CLI provider.")),
        }
        if !self.model.trim().is_empty() {
            cmd.arg("--model").arg(self.model.trim());
        }
        Ok(cmd)
    }
}

struct Reply {
    text: String,
    usage: Option<TokenUsage>,
}
fn parse_reply(kind: ProviderKind, bytes: &[u8]) -> Result<Reply, CoreError> {
    let malformed = || {
        failure(
            "Provider CLI returned an incomplete or unrecognized response; no text was replaced.",
        )
    };
    let mut reply = Reply {
        text: String::new(),
        usage: None,
    };
    if kind == ProviderKind::OpenCodeCli {
        let text = std::str::from_utf8(bytes).map_err(|_| malformed())?;
        let mut finished = false;
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let event: Value = serde_json::from_str(line).map_err(|_| malformed())?;
            match event["type"].as_str() {
                Some("text") => { reply.text.push_str(event.pointer("/part/text").and_then(Value::as_str).ok_or_else(malformed)?); }
                Some("step_finish") => {
                    if event.pointer("/part/reason").and_then(Value::as_str) != Some("stop") { return Err(malformed()); }
                    finished = true;
                    if let Some(tokens) = event.pointer("/part/tokens") {
                        reply.usage = Some(TokenUsage { input: tokens["input"].as_u64().unwrap_or(0), output: tokens["output"].as_u64().unwrap_or(0) });
                    }
                }
                Some("error" | "tool_use") => return Err(failure("Provider CLI could not complete this writing request. Check sign-in and model access; tools are disabled for rewrites.")),
                Some("step_start") => { finished = false; }
                _ => {}
            }
        }
        if !finished {
            return Err(malformed());
        }
    } else {
        let value: Value = serde_json::from_slice(bytes).map_err(|_| malformed())?;
        if value["type"] != "result"
            || value["subtype"] != "success"
            || value["is_error"].as_bool() == Some(true)
        {
            return Err(failure("Provider CLI did not complete this writing request. Check sign-in and model access; no text was replaced."));
        }
        if value
            .get("stop_reason")
            .and_then(Value::as_str)
            .is_some_and(|r| !["end_turn", "stop", "stop_sequence"].contains(&r))
        {
            return Err(malformed());
        }
        reply.text = value["result"].as_str().ok_or_else(malformed)?.to_string();
        if let Some(u) = value.get("usage") {
            reply.usage = Some(TokenUsage {
                input: u["input_tokens"].as_u64().unwrap_or(0),
                output: u["output_tokens"].as_u64().unwrap_or(0),
            });
        }
    }
    reply.text = reply.text.trim().to_string();
    if reply.text.is_empty() {
        return Err(malformed());
    }
    Ok(reply)
}

#[async_trait]
impl LlmProvider for CliProvider {
    async fn complete(&self, req: CompletionRequest) -> Result<String, CoreError> {
        let binary = resolve_binary(self.kind, self.binary.as_deref())?;
        let dir = tempfile::Builder::new()
            .prefix("selara-writing-")
            .tempdir()?;
        let cmd = self.command(&binary, dir.path())?;
        let input = format!("You are a text transformation service. Do not use tools. Return only the finished replacement text, with no commentary.\n\n{}\n\nText to transform:\n{}", req.system, req.user);
        let out = run_process(cmd, input.as_bytes(), RUN_TIMEOUT).await?;
        let reply = parse_reply(self.kind, &out)?;
        let kind = match self.kind {
            ProviderKind::ClaudeCli => "claude_cli",
            ProviderKind::CursorCli => "cursor_cli",
            _ => "open_code_cli",
        };
        usage::record_optional(kind, &self.model, "", reply.usage);
        Ok(reply.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_errors_partial_and_empty_results_without_echoing_output() {
        for input in [
            r#"{"type":"result","subtype":"error_max_turns","result":"secret partial"}"#,
            r#"{"type":"result","subtype":"success","is_error":true,"result":"secret partial"}"#,
            r#"{"type":"result","subtype":"success","stop_reason":"max_tokens","result":"secret partial"}"#,
            r#"{"type":"result","subtype":"success","result":" "}"#,
        ] {
            let err = parse_reply(ProviderKind::ClaudeCli, input.as_bytes())
                .err()
                .unwrap();
            assert!(!err.to_string().contains("secret"));
        }
        let good = br#"{"type":"result","subtype":"success","is_error":false,"result":" Done. ","usage":{"input_tokens":10,"output_tokens":2}}"#;
        let reply = parse_reply(ProviderKind::ClaudeCli, good).unwrap();
        assert_eq!(reply.text, "Done.");
        assert_eq!(reply.usage.unwrap().input, 10);
        assert_eq!(
            parse_reply(ProviderKind::CursorCli, good).unwrap().text,
            "Done."
        );
    }

    #[test]
    fn opencode_requires_terminal_success_and_rejects_tool_or_error_events() {
        let text = "{\"type\":\"text\",\"part\":{\"text\":\"Rewritten.\"}}\n";
        let finish = "{\"type\":\"step_finish\",\"part\":{\"reason\":\"stop\",\"tokens\":{\"input\":5,\"output\":2}}}\n";
        assert!(parse_reply(ProviderKind::OpenCodeCli, text.as_bytes()).is_err());
        assert_eq!(
            parse_reply(
                ProviderKind::OpenCodeCli,
                format!("{text}{finish}").as_bytes()
            )
            .unwrap()
            .text,
            "Rewritten."
        );
        for event in [
            "{\"type\":\"error\"}",
            "{\"type\":\"tool_use\"}",
            "{\"type\":\"step_start\"}",
        ] {
            assert!(parse_reply(
                ProviderKind::OpenCodeCli,
                format!("{text}{finish}{event}").as_bytes()
            )
            .is_err());
        }
        assert!(parse_reply(
            ProviderKind::OpenCodeCli,
            format!("{text}{}", finish.replace("stop", "length")).as_bytes()
        )
        .is_err());
    }

    #[test]
    fn each_command_has_isolated_settings_and_no_user_text_in_arguments() {
        for kind in [
            ProviderKind::ClaudeCli,
            ProviderKind::CursorCli,
            ProviderKind::OpenCodeCli,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let p = CliProvider::new(kind, "test-model".into(), None);
            let cmd = p.command(Path::new("/test/cli"), dir.path()).unwrap();
            let std = cmd.as_std();
            let args: Vec<_> = std
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            assert!(!args.iter().any(|a| [
                "--force",
                "--yolo",
                "--auto",
                "--dangerously-skip-permissions"
            ]
            .contains(&a.as_str())));
            assert_eq!(std.get_current_dir(), Some(dir.path()));
            assert!(std
                .get_envs()
                .any(|(key, val)| key == "PWD" && val == Some(dir.path().as_os_str())));
            match kind {
                ProviderKind::ClaudeCli => {
                    assert!(args.contains(&"--safe-mode".into()));
                    assert!(args.windows(2).any(|a| a == ["--disallowedTools", "*"]));
                    assert!(args.windows(2).any(|a| a == ["--tools", ""]));
                }
                ProviderKind::CursorCli => {
                    let cfg: Value = serde_json::from_slice(
                        &std::fs::read(dir.path().join("cursor-config/cli-config.json")).unwrap(),
                    )
                    .unwrap();
                    assert!(cfg["permissions"]["allow"].as_array().unwrap().is_empty());
                    assert!(cfg["permissions"]["deny"]
                        .as_array()
                        .unwrap()
                        .contains(&json!("Mcp(*:*)")));
                    assert!(args.windows(2).any(|a| a == ["--mode", "ask"]));
                }
                ProviderKind::OpenCodeCli => {
                    assert!(args.contains(&"--pure".into()));
                    assert!(std
                        .get_envs()
                        .any(|(key, val)| key == "OPENCODE_CONFIG_CONTENT"
                            && val.unwrap().to_string_lossy().contains("\"*\":\"deny\"")));
                }
                _ => unreachable!(),
            }
        }
    }

    #[cfg(unix)]
    fn script(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("fake provider");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn cursor_discovery_does_not_select_an_unrelated_agent() {
        let dir = tempfile::tempdir().unwrap();
        let executable = script(dir.path(), "exit 0");
        let generic = dir.path().join("agent");
        std::fs::rename(&executable, &generic).unwrap();
        let dirs = [dir.path().to_path_buf()];
        assert!(resolve_binary_in(ProviderKind::CursorCli, None, &dirs).is_err());
        assert_eq!(
            resolve_binary_in(ProviderKind::CursorCli, Some(Path::new("agent")), &dirs).unwrap(),
            generic
        );
        let cursor = dir.path().join("cursor-agent");
        std::fs::copy(&generic, &cursor).unwrap();
        assert_eq!(
            resolve_binary_in(ProviderKind::CursorCli, None, &dirs).unwrap(),
            cursor
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn process_round_trip_keeps_prompts_on_stdin_and_handles_paths_with_spaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = script(dir.path(), "if [ \"$1\" = '--version' ]; then printf '1.2.3\\n'; exit 0; fi\ncase \"$*\" in *PRIVATE_TEXT*) exit 7;; esac\ninput=$(cat)\ncase \"$input\" in *PRIVATE_TEXT*) ;; *) exit 8;; esac\nprintf '%s' '{\"type\":\"result\",\"subtype\":\"success\",\"result\":\"Corrected.\"}'");
        let status = inspect(ProviderKind::ClaudeCli, Some(path.clone()))
            .await
            .unwrap();
        assert!(status.installed);
        assert_eq!(status.version.as_deref(), Some("1.2.3"));
        let p = CliProvider::new(ProviderKind::ClaudeCli, "model".into(), Some(path));
        assert_eq!(
            p.complete(CompletionRequest {
                system: "Proofread.".into(),
                user: "PRIVATE_TEXT".into()
            })
            .await
            .unwrap(),
            "Corrected."
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn nonzero_exit_and_excessive_output_fail_without_leaking_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let path = script(dir.path(), "printf 'private prompt' >&2; exit 3");
        let error = run_process(
            Command::new(&path),
            b"private prompt",
            Duration::from_secs(2),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("3"));
        assert!(!error.to_string().contains("private prompt"));
        let path = script(dir.path(), "head -c 5000000 /dev/zero");
        assert!(run_process(Command::new(path), b"", Duration::from_secs(2))
            .await
            .unwrap_err()
            .to_string()
            .contains("size limit"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_and_cancellation_kill_the_child_process_group() {
        for cancel in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = script(dir.path(), "(sleep 0.3; printf leaked > leaked) &\nwait");
            let mut cmd = Command::new(path);
            cmd.current_dir(dir.path());
            if cancel {
                let task =
                    tokio::spawn(
                        async move { run_process(cmd, b"", Duration::from_secs(2)).await },
                    );
                tokio::time::sleep(Duration::from_millis(50)).await;
                task.abort();
                let _ = task.await;
            } else {
                assert!(run_process(cmd, b"", Duration::from_millis(50))
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("timed out"));
            }
            tokio::time::sleep(Duration::from_millis(350)).await;
            assert!(!dir.path().join("leaked").exists());
        }
    }
}
