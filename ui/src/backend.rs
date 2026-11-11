//! Background IPC. The UI thread never waits for Lua, files, or device requests.
use crate::control::Request;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    net::TcpStream,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};
mod commands;
mod lifecycle;
use commands::spawn_worker;
#[derive(Clone)]
pub struct Client {
    queue: mpsc::SyncSender<Value>,
    execution: mpsc::SyncSender<Value>,
    pub endpoint: PathBuf,
    latest_ui: Arc<Mutex<Option<Value>>>,
}
fn send(sender: &async_channel::Sender<Request>, body: Value) {
    let (tx, _) = mpsc::sync_channel(1);
    let _ = sender.send_blocking(Request {
        body,
        reply: tx,
        expires: None,
    });
}
pub fn endpoint() -> PathBuf {
    std::env::var_os("PERIPHERAL_BACKEND_ENDPOINT")
        .map(PathBuf::from)
        .unwrap_or_else(peripheral_repl::protocol::default_endpoint)
}
pub fn data_dir() -> PathBuf {
    std::env::var_os("PERIPHERAL_UI_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default())
                .join("peripheral-repl/ui")
        })
}
pub fn workspace() -> PathBuf {
    let saved = std::fs::read(data_dir().join("workspace.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v["path"].as_str().map(PathBuf::from));
    saved
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}
pub fn save_workspace(path: &Path) -> Result<()> {
    let path = path.canonicalize()?;
    std::fs::create_dir_all(data_dir())?;
    std::fs::write(
        data_dir().join("workspace.json"),
        serde_json::to_vec(&json!({"path":path}))?,
    )?;
    Ok(())
}
fn request(endpoint: &Path, command: Value) -> Result<Value> {
    let response = peripheral_repl::protocol::request(endpoint, command)?;
    anyhow::ensure!(
        response["ok"] == true,
        "{}",
        response["error"]["message"]
            .as_str()
            .unwrap_or("backend request failed")
    );
    Ok(response["result"].clone())
}
fn refresh(endpoint: &Path) -> Result<Value> {
    let ping = request(endpoint, json!({"op":"ping"}))?;
    anyhow::ensure!(
        ping["ui_protocol"] == peripheral_ui_protocol::VERSION && ping["plugin_api"] == 2,
        "UI/backend protocol mismatch (UI requires {}, backend reports {}). Restart both from the same application version, or connect to a compatible endpoint.",
        peripheral_ui_protocol::VERSION,
        ping["ui_protocol"]
    );
    let names = request(endpoint, json!({"op":"instances"}))?;
    let mut sessions = Vec::new();
    for name in names.as_array().into_iter().flatten() {
        let stats = request(endpoint, json!({"op":"stats","instance":name})).unwrap_or(Value::Null);
        let tasks = request(endpoint, json!({"op":"tasks","instance":name})).unwrap_or(Value::Null);
        sessions.push(json!({"name":name,"stats":stats,"tasks":tasks}));
    }
    Ok(
        json!({"connection":"connected","ping":ping,"sessions":sessions,"plugins":request(endpoint,json!({"op":"plugins"}))?,"ui":request(endpoint,json!({"op":"ui_snapshot"}))?}),
    )
}
impl Client {
    pub fn new(sender: async_channel::Sender<Request>) -> Self {
        let endpoint = endpoint();
        let workspace = workspace();
        let (tx, rx) = mpsc::sync_channel::<Value>(16);
        let selected = Arc::new(Mutex::new(workspace.clone()));
        let (execution, exec_rx) = mpsc::sync_channel(8);
        spawn_worker(rx, endpoint.clone(), sender.clone(), selected.clone());
        spawn_worker(exec_rx, endpoint.clone(), sender.clone(), selected);
        let ep = endpoint.clone();
        let latest_ui = Arc::new(Mutex::new(None));
        let pending_ui = latest_ui.clone();
        std::thread::spawn(move || {
            let mut disconnected = None;
            loop {
                let result = (|| -> Result<()> {
                    let endpoint: peripheral_repl::protocol::Endpoint =
                        serde_json::from_slice(&std::fs::read(&ep)?)?;
                    anyhow::ensure!(endpoint.address.ip().is_loopback(), "backend must be local");
                    let mut stream =
                        TcpStream::connect_timeout(&endpoint.address, Duration::from_secs(2))?;
                    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
                    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
                    serde_json::to_writer(
                        &mut stream,
                        &json!({"token":endpoint.token,"command":{"op":"event_stream","after":0}}),
                    )?;
                    stream.write_all(b"\n")?;
                    send(&sender, json!({"op":"backend_state","state":refresh(&ep)?}));
                    disconnected = None;
                    let mut reader = BufReader::new(stream);
                    loop {
                        let mut line = String::new();
                        let n = std::io::Read::take(&mut reader, 256 * 1024 + 1)
                            .read_line(&mut line)?;
                        anyhow::ensure!(
                            n > 0 && n <= 256 * 1024 && line.ends_with('\n'),
                            "backend stream interrupted"
                        );
                        let event: Value = serde_json::from_str(&line)?;
                        anyhow::ensure!(event["ok"] == true, "backend stream rejected");
                        let events = event["result"]["events"].as_array().context("event page")?;
                        let visible = events
                            .iter()
                            .filter(|e| !e["kind"].as_str().is_some_and(|s| s.starts_with("ui.")))
                            .collect::<Vec<_>>();
                        if !visible.is_empty() {
                            send(&sender, json!({"op":"backend_events","events":visible}));
                        }
                        if event["result"]["gap"] == true
                            || events.iter().any(|e| {
                                e["kind"].as_str().is_some_and(|s| {
                                    s.starts_with("session.")
                                        || s.starts_with("plugin.")
                                        || s == "config.defined"
                                        || s == "config"
                                })
                            })
                        {
                            send(&sender, json!({"op":"backend_state","state":refresh(&ep)?}));
                        }
                        if event["result"]["gap"] == true
                            || events
                                .iter()
                                .any(|e| e["kind"].as_str().is_some_and(|s| s.starts_with("ui.")))
                        {
                            let snapshot = request(&ep, json!({"op":"ui_snapshot"}))?;
                            let wake = pending_ui.lock().unwrap().replace(snapshot).is_none();
                            if wake {
                                send(&sender, json!({"op":"backend_ui_ready"}));
                            }
                        }
                    }
                })();
                let alive = std::fs::read(&ep)
                    .ok()
                    .and_then(|b| {
                        serde_json::from_slice::<peripheral_repl::protocol::Endpoint>(&b).ok()
                    })
                    .is_some_and(|e| crate::native::process_alive(e.pid));
                if disconnected != Some(alive) {
                    send(
                        &sender,
                        json!({"op":"backend_disconnected","alive":alive,"error":result.err().map(|e|format!("{e:#}"))}),
                    );
                    disconnected = Some(alive);
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        });
        Self {
            queue: tx,
            execution,
            endpoint,
            latest_ui,
        }
    }
    pub fn take_ui(&self) -> Option<Value> {
        self.latest_ui.lock().unwrap().take()
    }
    pub fn command(&self, body: Value) -> Result<()> {
        let lane = if matches!(
            body["op"].as_str(),
            Some("eval" | "load" | "invoke" | "ui_event")
        ) {
            &self.execution
        } else {
            &self.queue
        };
        lane.try_send(body).context("backend command queue full")?;
        Ok(())
    }
}
pub fn run_daemon(root: &Path) -> Result<()> {
    let root = root.canonicalize()?;
    let mut runtime = peripheral_repl::runtime::Runtime::new(&root)?;
    // Host-owned policy, never supplied or widened by an installed Lua plugin.
    // A missing file stays read-only; malformed policies fail startup visibly.
    let policy = root.join(".peripheral/policy.json");
    if policy.exists() {
        anyhow::ensure!(
            policy.canonicalize()?.starts_with(&root),
            "host policy escapes workspace"
        );
        Arc::get_mut(&mut runtime.shared)
            .context("runtime already shared")?
            .policy = peripheral_repl::bindings::load_policy(&policy)?;
    }
    runtime.dispatch(json!({"op":"create","instance":"default"}))?;
    peripheral_repl::protocol::serve(runtime, &endpoint(), root, None)
}
