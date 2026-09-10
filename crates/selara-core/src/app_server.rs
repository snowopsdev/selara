//! Supervised JSON-RPC connection to Selara's bundled writing runtime.
//! Authentication storage and transport stay inside Codex, never in Selara.
use crate::error::CoreError;
use crate::usage::TokenUsage;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{broadcast, oneshot, Mutex};

const RPC_TIMEOUT: Duration = Duration::from_secs(30);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(600);
const TURN_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
fn error(message: impl Into<String>) -> CoreError {
    CoreError::Provider(message.into())
}
fn lock<T>(mutex: &StdMutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn resolve_executable() -> Result<PathBuf, CoreError> {
    if std::env::var_os("CODEX_BIN").is_some_and(|v| !v.is_empty()) {
        return Err(CoreError::Config("Remove legacy CODEX_BIN; this release uses its bundled writing runtime. Set provider.codex_home to select your existing account store.".into()));
    }
    let executable = std::env::current_exe()?.canonicalize()?;
    resolve_beside(&executable).ok_or_else(|| CoreError::Config("The bundled Codex runtime is missing. Reinstall the complete Selara app or CLI archive.".into()))
}
fn resolve_beside(executable: &Path) -> Option<PathBuf> {
    let directory = executable.parent()?;
    [
        directory.join("selara-codex"),
        directory.join("../libexec/selara-codex"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}
static HOME: OnceLock<StdMutex<Option<PathBuf>>> = OnceLock::new();
pub fn configure_home(home: Option<&Path>) {
    *lock(HOME.get_or_init(|| StdMutex::new(None))) = home.map(Path::to_path_buf);
}
fn home() -> Result<PathBuf, CoreError> {
    resolve_home(lock(HOME.get_or_init(|| StdMutex::new(None))).as_deref())
}
pub fn resolve_home(configured: Option<&Path>) -> Result<PathBuf, CoreError> {
    if std::env::var_os("CODEX_AUTH_JSON").is_some_and(|v| !v.is_empty()) {
        return Err(CoreError::Config("CODEX_AUTH_JSON is no longer supported. Select the existing Codex home with provider.codex_home or CODEX_HOME; do not copy tokens into Selara.".into()));
    }
    let path = configured
        .map(Path::to_path_buf)
        .or_else(|| {
            std::env::var_os("CODEX_HOME")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        })
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".codex")))
        .ok_or_else(|| CoreError::Config("Could not locate your Codex home".into()))?;
    if !path.is_absolute() {
        return Err(CoreError::Config(
            "provider.codex_home must be an absolute path".into(),
        ));
    }
    Ok(path)
}

type Reply = Result<Value, String>;
struct Client {
    child: Mutex<Child>,
    input: Mutex<ChildStdin>,
    pending: StdMutex<HashMap<u64, oneshot::Sender<Reply>>>,
    events: broadcast::Sender<Value>,
    next_id: AtomicU64,
    dead: AtomicBool,
    login_id: StdMutex<Option<String>>,
    login_cancelled: AtomicBool,
}
struct PendingRequest {
    client: Arc<Client>,
    id: u64,
}
impl Drop for PendingRequest {
    fn drop(&mut self) {
        lock(&self.client.pending).remove(&self.id);
    }
}
impl Client {
    async fn spawn(program: &Path, args: &[&str], home: &Path) -> Result<Arc<Self>, CoreError> {
        let mut child = Command::new(program)
            .args(args)
            .env("CODEX_HOME", home)
            .current_dir(std::env::temp_dir())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| error(format!("Could not start the bundled Codex runtime: {e}")))?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| error("Codex stdin unavailable"))?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| error("Codex stdout unavailable"))?;
        let (events, _) = broadcast::channel(256);
        let client = Arc::new(Self {
            child: Mutex::new(child),
            input: Mutex::new(input),
            pending: StdMutex::new(HashMap::new()),
            events,
            next_id: AtomicU64::new(1),
            dead: AtomicBool::new(false),
            login_id: StdMutex::new(None),
            login_cancelled: AtomicBool::new(false),
        });
        let weak = Arc::downgrade(&client);
        tokio::spawn(async move {
            let mut output = BufReader::new(output);
            loop {
                let mut line = Vec::new();
                let result = (&mut output)
                    .take((MAX_FRAME_BYTES + 1) as u64)
                    .read_until(b'\n', &mut line)
                    .await;
                let Some(client) = weak.upgrade() else {
                    break;
                };
                if result.is_err()
                    || line.is_empty()
                    || line.len() > MAX_FRAME_BYTES
                    || !line.ends_with(b"\n")
                {
                    client.fail("Codex runtime exited or returned an oversized response");
                    let _ = client.child.lock().await.kill().await;
                    break;
                }
                let value: Value = match serde_json::from_slice(&line) {
                    Ok(value) => value,
                    Err(_) => {
                        client.fail("Codex runtime returned an invalid response");
                        let _ = client.child.lock().await.kill().await;
                        break;
                    }
                };
                if let Some(id) = value.get("id").and_then(Value::as_u64) {
                    if let Some(reply) = lock(&client.pending).remove(&id) {
                        let result = match value.get("error") {
                            Some(err) => Err(err
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("Codex request failed")
                                .to_owned()),
                            None => Ok(value.get("result").cloned().unwrap_or(Value::Null)),
                        };
                        let _ = reply.send(result);
                    }
                } else if value.get("method").is_some() {
                    let _ = client.events.send(value);
                }
            }
        });
        let initialized = client.request("initialize", json!({"clientInfo":{"name":"selara","version":env!("CARGO_PKG_VERSION")},"capabilities":{},"selaraWritingMode":1})).await?;
        if initialized.get("selaraWritingMode").and_then(Value::as_u64) != Some(1) {
            client.stop().await;
            return Err(error(
                "This Codex binary does not enforce Selara writing mode. Reinstall Selara.",
            ));
        }
        client.notify("initialized", json!({})).await?;
        Ok(client)
    }
    fn fail(&self, message: &str) {
        self.dead.store(true, Ordering::SeqCst);
        for (_, reply) in lock(&self.pending).drain() {
            let _ = reply.send(Err(message.into()));
        }
        let _ = self
            .events
            .send(json!({"method":"selara/runtimeExited","params":{"message":message}}));
    }
    async fn stop(&self) {
        self.fail("Codex runtime restarted; retry the request");
        let _ = self.child.lock().await.kill().await;
    }
    async fn write(&self, value: Value) -> Result<(), CoreError> {
        if self.dead.load(Ordering::SeqCst) {
            return Err(error("Codex runtime is no longer running"));
        }
        let mut bytes =
            serde_json::to_vec(&value).map_err(|_| error("Could not encode Codex request"))?;
        bytes.push(b'\n');
        let mut input = self.input.lock().await;
        input.write_all(&bytes).await?;
        input.flush().await?;
        Ok(())
    }
    async fn notify(&self, method: &str, params: Value) -> Result<(), CoreError> {
        self.write(json!({"method":method,"params":params})).await
    }
    async fn request(self: &Arc<Self>, method: &str, params: Value) -> Result<Value, CoreError> {
        self.request_timeout(method, params, RPC_TIMEOUT).await
    }
    async fn request_timeout(
        self: &Arc<Self>,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, CoreError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (send, receive) = oneshot::channel();
        lock(&self.pending).insert(id, send);
        let _guard = PendingRequest {
            client: self.clone(),
            id,
        };
        tokio::time::timeout(timeout, async {
            self.write(json!({"id":id,"method":method,"params":params}))
                .await?;
            receive
                .await
                .map_err(|_| error("Codex runtime disconnected"))?
                .map_err(error)
        })
        .await
        .map_err(|_| {
            error(format!(
                "Codex {method} timed out; retry when the service is available"
            ))
        })?
    }
}

type RuntimeSlot = Option<(PathBuf, Arc<Client>)>;
static CLIENT: OnceLock<Mutex<RuntimeSlot>> = OnceLock::new();
async fn client() -> Result<Arc<Client>, CoreError> {
    client_for(home()?).await
}
async fn client_for(desired_home: PathBuf) -> Result<Arc<Client>, CoreError> {
    let mut slot = CLIENT.get_or_init(|| Mutex::new(None)).lock().await;
    if let Some((current_home, connection)) = &*slot {
        if current_home == &desired_home && !connection.dead.load(Ordering::SeqCst) {
            return Ok(connection.clone());
        }
        connection.stop().await;
    }
    *slot = None;
    let connection = Client::spawn(
        &resolve_executable()?,
        &[
            "app-server",
            "--listen",
            "stdio://",
            "--selara-writing-mode",
        ],
        &desired_home,
    )
    .await?;
    *slot = Some((desired_home, connection.clone()));
    Ok(connection)
}
pub async fn reset() -> Result<(), CoreError> {
    if let Some((_, connection)) = CLIENT.get_or_init(|| Mutex::new(None)).lock().await.take() {
        connection.stop().await;
    }
    Ok(())
}
pub async fn status() -> Result<Value, CoreError> {
    client()
        .await?
        .request("account/read", json!({"refreshToken":false}))
        .await
}

pub struct LoginFlow {
    pub auth_url: String,
    id: String,
    client: Arc<Client>,
    events: broadcast::Receiver<Value>,
}
impl LoginFlow {
    pub async fn wait(mut self) -> Result<(), CoreError> {
        let result = tokio::time::timeout(LOGIN_TIMEOUT, async {
            loop {
                let event = self
                    .events
                    .recv()
                    .await
                    .map_err(|_| error("Login connection was interrupted"))?;
                if event["method"] == "selara/runtimeExited" {
                    return Err(error("Codex runtime exited during login"));
                }
                if event["method"] == "account/login/completed"
                    && event["params"]["loginId"] == self.id
                {
                    return if event["params"]["success"] == true {
                        Ok(())
                    } else {
                        Err(error(
                            event["params"]["error"]
                                .as_str()
                                .unwrap_or("Sign-in cancelled or failed"),
                        ))
                    };
                }
            }
        })
        .await
        .unwrap_or_else(|_| Err(error("Browser sign-in timed out")));
        if result.is_err() {
            let _ = self
                .client
                .request("account/login/cancel", json!({"loginId":self.id}))
                .await;
        }
        let mut active = lock(&self.client.login_id);
        if active.as_deref() == Some(&self.id) {
            *active = None;
        }
        result
    }
}
async fn begin_login_with(connection: Arc<Client>) -> Result<LoginFlow, CoreError> {
    {
        let mut active = lock(&connection.login_id);
        if active.is_some() {
            return Err(error("Sign-in is already in progress"));
        }
        connection.login_cancelled.store(false, Ordering::SeqCst);
        *active = Some(String::new());
    }
    let events = connection.events.subscribe();
    let started = connection
        .request("account/login/start", json!({"type":"chatgpt"}))
        .await;
    let flow_id = started
        .as_ref()
        .ok()
        .and_then(|v| v["loginId"].as_str())
        .map(str::to_owned);
    let result = (|| {
        let value = started?;
        let id = value["loginId"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| error("Codex did not return a login id"))?
            .to_owned();
        let url = value["authUrl"]
            .as_str()
            .ok_or_else(|| error("Codex did not return a browser sign-in URL"))?;
        let parsed =
            reqwest::Url::parse(url).map_err(|_| error("Codex returned an invalid sign-in URL"))?;
        if parsed.scheme() != "https"
            || !matches!(
                parsed.host_str(),
                Some("auth.openai.com" | "auth.chatgpt.com")
            )
        {
            return Err(error("Codex returned an unexpected sign-in destination"));
        }
        let mut active = lock(&connection.login_id);
        if connection.login_cancelled.load(Ordering::SeqCst) {
            return Err(error("Sign-in cancelled"));
        }
        *active = Some(id.clone());
        Ok(LoginFlow {
            auth_url: url.to_owned(),
            id,
            client: connection.clone(),
            events,
        })
    })();
    if result.is_err() {
        if let Some(id) = flow_id {
            let _ = connection
                .request("account/login/cancel", json!({"loginId":id}))
                .await;
        }
        *lock(&connection.login_id) = None;
    }
    result
}
pub async fn begin_login() -> Result<LoginFlow, CoreError> {
    begin_login_with(client().await?).await
}
async fn cancel_login_with(connection: Arc<Client>) -> Result<(), CoreError> {
    let id = {
        let active = lock(&connection.login_id);
        let id = active
            .clone()
            .ok_or_else(|| error("No browser sign-in is in progress"))?;
        connection.login_cancelled.store(true, Ordering::SeqCst);
        id
    };
    // Login start may still be returning its id. Its continuation observes the
    // cancellation under the same lock, then cancels before exposing a URL.
    if id.is_empty() {
        return Ok(());
    }
    connection
        .request("account/login/cancel", json!({"loginId":id}))
        .await?;
    let _ = connection.events.send(json!({"method":"account/login/completed","params":{"loginId":id,"success":false,"error":"Sign-in cancelled"}}));
    Ok(())
}
pub async fn cancel_login() -> Result<(), CoreError> {
    cancel_login_with(client().await?).await
}
pub async fn logout() -> Result<(), CoreError> {
    client().await?.request("account/logout", json!({})).await?;
    reset().await
}

pub async fn models() -> Result<Vec<String>, CoreError> {
    models_with(client().await?).await
}
async fn models_with(connection: Arc<Client>) -> Result<Vec<String>, CoreError> {
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    let mut seen = HashSet::new();
    loop {
        let value = connection
            .request("model/list", json!({"cursor":cursor}))
            .await?;
        let data = value["data"]
            .as_array()
            .ok_or_else(|| error("Codex returned an invalid model list"))?;
        for model in data {
            if model["hidden"] == true {
                continue;
            }
            if let Some(id) = model["id"].as_str() {
                if !out.iter().any(|s| s == id) {
                    out.push(id.to_owned());
                }
            }
        }
        cursor = value["nextCursor"].as_str().map(str::to_owned);
        if let Some(next) = &cursor {
            if !seen.insert(next.clone()) || seen.len() > 100 {
                return Err(error("Codex model pagination did not finish"));
            }
        } else {
            break;
        }
    }
    Ok(out)
}

pub struct Completion {
    pub text: String,
    pub usage: Option<TokenUsage>,
}
struct TurnGuard {
    client: Arc<Client>,
    thread: String,
    turn: String,
    finished: bool,
}
impl Drop for TurnGuard {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let client = self.client.clone();
        let thread = self.thread.clone();
        let turn = self.turn.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if !turn.is_empty() {
                    let _ = client
                        .request("turn/interrupt", json!({"threadId":thread,"turnId":turn}))
                        .await;
                }
                let _ = client
                    .request("thread/unsubscribe", json!({"threadId":thread}))
                    .await;
            });
        }
    }
}
async fn complete_with(
    connection: Arc<Client>,
    model: &str,
    system: &str,
    user: &str,
) -> Result<Completion, CoreError> {
    let mut events = connection.events.subscribe();
    let thread = connection
        .request(
            "thread/start",
            json!({"ephemeral":true,"model":model,"baseInstructions":system}),
        )
        .await?;
    let thread_id = thread["thread"]["id"]
        .as_str()
        .ok_or_else(|| error("Codex returned no thread id"))?
        .to_owned();
    let mut guard = TurnGuard {
        client: connection.clone(),
        thread: thread_id.clone(),
        turn: String::new(),
        finished: false,
    };
    let started = connection
        .request(
            "turn/start",
            json!({"threadId":thread_id,"input":[{"type":"text","text":user}]}),
        )
        .await?;
    let turn_id = started["turn"]["id"]
        .as_str()
        .ok_or_else(|| error("Codex returned no turn id"))?
        .to_owned();
    guard.turn = turn_id.clone();
    let result = tokio::time::timeout(TURN_TIMEOUT, async {
        let mut items: Vec<(String, String)> = Vec::new();
        let mut usage = None;
        loop {
            let event = events
                .recv()
                .await
                .map_err(|_| error("Codex response stream was interrupted; nothing was written"))?;
            if event["method"] == "selara/runtimeExited" {
                return Err(error("Codex exited before finishing; nothing was written"));
            }
            let params = &event["params"];
            if params["threadId"] != thread_id {
                continue;
            }
            if event["method"] == "thread/tokenUsage/updated" {
                let last = &params["tokenUsage"]["last"];
                usage = Some(TokenUsage {
                    input: last["inputTokens"].as_u64().unwrap_or(0),
                    output: last["outputTokens"].as_u64().unwrap_or(0),
                });
            }
            if params["turnId"] == turn_id && event["method"] == "item/completed" {
                let item = &params["item"];
                if item["type"] != "agentMessage" {
                    return Err(error(
                        "Codex returned a non-writing item; nothing was written",
                    ));
                }
                let id = item["id"]
                    .as_str()
                    .ok_or_else(|| error("Codex returned an invalid text item"))?;
                let text = item["text"]
                    .as_str()
                    .ok_or_else(|| error("Codex returned no completed text"))?;
                if let Some(existing) = items.iter_mut().find(|(key, _)| key == id) {
                    existing.1 = text.into();
                } else {
                    items.push((id.into(), text.into()));
                }
            }
            if event["method"] == "turn/completed" && params["turn"]["id"] == turn_id {
                if params["turn"]["status"] != "completed" {
                    return Err(error(
                        "Codex did not complete the request; nothing was written",
                    ));
                }
                let text = items
                    .into_iter()
                    .map(|(_, text)| text)
                    .collect::<Vec<_>>()
                    .join("\n")
                    .trim()
                    .to_owned();
                if text.is_empty() {
                    return Err(error(
                        "Codex completed without output text; nothing was written",
                    ));
                }
                return Ok(Completion { text, usage });
            }
        }
    })
    .await
    .map_err(|_| error("Codex response timed out; nothing was written"))?;
    if result.is_ok() {
        guard.finished = true;
    }
    let _ = connection
        .request("thread/unsubscribe", json!({"threadId":thread_id}))
        .await;
    result
}
pub async fn complete_at(
    home: &Path,
    model: &str,
    system: &str,
    user: &str,
) -> Result<Completion, CoreError> {
    complete_with(client_for(home.to_path_buf()).await?, model, system, user).await
}

#[cfg(test)]
mod tests {
    use super::*;
    const FIXTURE: &str = include_str!("../tests/fixtures/app_server.py");
    async fn fixture(mode: &str) -> Arc<Client> {
        Client::spawn(
            Path::new("python3"),
            &["-u", "-c", FIXTURE, mode],
            &std::env::temp_dir(),
        )
        .await
        .unwrap()
    }
    #[tokio::test]
    #[ignore = "requires the locally built native runtime; CI runs this after packaging"]
    async fn native_runtime_handshake_and_signed_out_account() {
        let program = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/selara-codex");
        let directory = std::env::temp_dir().join(format!(
            "selara-native-client-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let client = Client::spawn(
            &program,
            &[
                "app-server",
                "--listen",
                "stdio://",
                "--selara-writing-mode",
            ],
            &directory,
        )
        .await
        .unwrap();
        let status = client
            .request("account/read", json!({"refreshToken":false}))
            .await
            .unwrap();
        assert!(status["account"].is_null());
        client.stop().await;
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[tokio::test]
    async fn requires_writing_capability_before_other_operations() {
        let result = Client::spawn(
            Path::new("python3"),
            &["-u", "-c", FIXTURE, "wrong_capability"],
            &std::env::temp_dir(),
        )
        .await;
        assert!(result.err().unwrap().to_string().contains("writing mode"));
    }
    #[tokio::test]
    async fn routes_concurrent_replies_and_unsolicited_events_independently() {
        let client = fixture("normal").await;
        let mut events = client.events.subscribe();
        let (slow, fast) = tokio::join!(
            client.request("test/echo", json!({"value":"slow","delay":0.1})),
            client.request("test/echo", json!({"value":"fast"}))
        );
        assert_eq!(slow.unwrap(), "slow");
        assert_eq!(fast.unwrap(), "fast");
        assert_eq!(events.recv().await.unwrap()["method"], "account/updated");
        assert!(lock(&client.pending).is_empty());
        client.stop().await;
    }
    #[tokio::test]
    async fn requests_are_bounded_and_runtime_exit_fails_pending_work() {
        let client = fixture("normal").await;
        let error = client
            .request_timeout("test/hang", json!({}), Duration::from_millis(30))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(lock(&client.pending).is_empty());
        assert!(client.request("test/exit", json!({})).await.is_err());
        assert!(client.dead.load(Ordering::SeqCst));
    }
    #[tokio::test]
    async fn login_completion_and_cancellation_are_not_lost_between_replies() {
        let client = fixture("login_complete").await;
        begin_login_with(client.clone())
            .await
            .unwrap()
            .wait()
            .await
            .unwrap();
        assert!(lock(&client.login_id).is_none());
        client.stop().await;
        let client = fixture("normal").await;
        let flow = begin_login_with(client.clone()).await.unwrap();
        cancel_login_with(client.clone()).await.unwrap();
        assert!(flow
            .wait()
            .await
            .unwrap_err()
            .to_string()
            .contains("cancelled"));
        client.stop().await;
    }
    #[tokio::test]
    async fn cancellation_during_login_start_cancels_before_exposing_browser_url() {
        let client = fixture("slow_login").await;
        let mut events = client.events.subscribe();
        let c = client.clone();
        let start = tokio::spawn(async move { begin_login_with(c).await });
        tokio::task::yield_now().await;
        cancel_login_with(client.clone()).await.unwrap();
        assert!(start
            .await
            .unwrap()
            .err()
            .unwrap()
            .to_string()
            .contains("cancelled"));
        assert_eq!(events.recv().await.unwrap()["method"], "fixture/cancelled");
        client.stop().await;
    }
    #[tokio::test]
    async fn invalid_browser_destination_cancels_the_created_flow() {
        let client = fixture("bad_url").await;
        let mut events = client.events.subscribe();
        assert!(begin_login_with(client.clone()).await.is_err());
        assert_eq!(events.recv().await.unwrap()["method"], "fixture/cancelled");
        assert!(lock(&client.login_id).is_none());
        client.stop().await;
    }
    #[tokio::test]
    async fn models_paginate_without_duplicates_or_hidden_models_and_reject_cycles() {
        let client = fixture("normal").await;
        assert_eq!(models_with(client.clone()).await.unwrap(), ["one", "two"]);
        client.stop().await;
        let client = fixture("broken_models").await;
        assert!(models_with(client.clone())
            .await
            .unwrap_err()
            .to_string()
            .contains("pagination"));
        client.stop().await;
    }
    #[tokio::test]
    async fn only_complete_successful_text_can_be_returned_for_replacement() {
        let client = fixture("normal").await;
        let result = complete_with(client.clone(), "model", "system", "selection")
            .await
            .unwrap();
        assert_eq!(result.text, "Complete rewrite");
        assert_eq!(result.usage.unwrap().output, 4);
        client.stop().await;
        for mode in [
            "failed",
            "interrupted",
            "incomplete",
            "empty",
            "tool",
            "disconnect",
            "start_failure",
        ] {
            let client = fixture(mode).await;
            assert!(
                complete_with(client.clone(), "model", "system", "selection")
                    .await
                    .is_err(),
                "{mode}"
            );
            client.stop().await;
        }
    }
    #[tokio::test]
    async fn failed_turn_start_releases_its_ephemeral_thread() {
        let client = fixture("start_failure").await;
        let mut events = client.events.subscribe();
        assert!(
            complete_with(client.clone(), "model", "system", "selection")
                .await
                .is_err()
        );
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event["params"]["method"], "thread/unsubscribe");
        client.stop().await;
    }
}
