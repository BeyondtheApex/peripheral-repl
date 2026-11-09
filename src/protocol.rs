use crate::runtime::Runtime;
use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    time::Duration,
};

pub const MAX_FRAME: usize = 256 * 1024;

// An absolute handshake deadline prevents a slow sender from extending the
// timeout indefinitely by trickling one byte per socket read timeout.
fn read_connection_frame(stream: &mut TcpStream) -> Result<String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut bytes = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            bail!("connection frame deadline exceeded");
        }
        stream.set_read_timeout(Some(remaining))?;
        let mut chunk = [0u8; 4096];
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            bail!("truncated connection frame");
        }
        let newline = chunk[..n].iter().position(|b| *b == b'\n');
        let end = newline.map_or(n, |i| i + 1);
        if bytes.len() + end > MAX_FRAME {
            bail!("frame exceeds 256 KiB");
        }
        bytes.extend_from_slice(&chunk[..end]);
        if newline.is_some() {
            return Ok(String::from_utf8(bytes)?);
        }
    }
}

pub fn read_frame(reader: &mut impl BufRead) -> Result<Option<String>> {
    let mut bytes = Vec::new();
    let n = reader
        .take((MAX_FRAME + 1) as u64)
        .read_until(b'\n', &mut bytes)?;
    if n == 0 {
        return Ok(None);
    }
    if n > MAX_FRAME {
        bail!("frame exceeds 256 KiB");
    }
    if bytes.last() != Some(&b'\n') {
        bail!("truncated frame: newline required");
    }
    Ok(Some(String::from_utf8(bytes)?))
}

pub fn envelope(runtime: &Runtime, request: Value) -> Value {
    let id = request
        .get("id")
        .cloned()
        .filter(|id| id.is_number() || id.is_null() || id.as_str().is_some_and(|s| s.len() <= 128))
        .unwrap_or(Value::Null);
    match runtime.dispatch(request) {
        Ok(result) if result.to_string().len() <= 128 * 1024 => {
            json!({"id":id,"ok":true,"result":result})
        }
        Ok(_) => {
            json!({"id":id,"ok":false,"error":{"message":"response exceeds 128 KiB; narrow the filter or return fewer values"}})
        }
        Err(e) => {
            json!({"id":id,"ok":false,"error":crate::error::json_error(&e)})
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct Endpoint {
    pub address: SocketAddr,
    pub token: String,
    pub pid: u32,
    pub root: PathBuf,
}

pub fn default_endpoint() -> PathBuf {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .expect("LOCALAPPDATA is required");
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").expect("HOME is required")).join(".local/state")
        });
    // Retain a running legacy endpoint; fresh installations reserve a dedicated
    // private directory, independent of GUI logs/preferences created beforehand.
    let legacy = base.join("peripheral-repl/session.json");
    if legacy.exists() {
        legacy
    } else {
        base.join("peripheral-repl/backend/session.json")
    }
}

pub fn request(endpoint: &Path, command: Value) -> Result<Value> {
    let endpoint: Endpoint = serde_json::from_str(&std::fs::read_to_string(endpoint)?)?;
    if !endpoint.address.ip().is_loopback() {
        bail!("endpoint must be loopback");
    }
    let mut stream = TcpStream::connect_timeout(&endpoint.address, Duration::from_secs(2))?;
    stream.set_read_timeout(Some(Duration::from_secs(130)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    serde_json::to_writer(
        &mut stream,
        &json!({"token":endpoint.token,"command":command}),
    )?;
    stream.write_all(b"\n")?;
    let line = read_frame(&mut BufReader::new(stream))?
        .ok_or_else(|| anyhow!("daemon closed without reply"))?;
    Ok(serde_json::from_str(&line)?)
}

pub fn watch(path: &Path, after: u64) -> Result<()> {
    let endpoint: Endpoint = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    if !endpoint.address.ip().is_loopback() {
        bail!("endpoint must be loopback");
    }
    let mut stream = TcpStream::connect_timeout(&endpoint.address, Duration::from_secs(2))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    serde_json::to_writer(
        &mut stream,
        &json!({"token":endpoint.token,"command":{"op":"event_stream","after":after}}),
    )?;
    stream.write_all(b"\n")?;
    let mut reader = BufReader::new(stream);
    let mut output = std::io::stdout().lock();
    while let Some(line) = read_frame(&mut reader)? {
        output.write_all(line.as_bytes())?;
        output.flush()?;
        if serde_json::from_str::<Value>(&line)?["ok"] != true {
            bail!("event stream rejected");
        }
    }
    Ok(())
}

/// Independent bounded requests let MCP cancellation/ping run during a Lua IO wait.
pub fn mcp_stdio(runtime: Runtime, endpoint: Option<PathBuf>) -> Result<()> {
    mcp_transport(Some(runtime), endpoint)
}

/// Connect to an existing backend without constructing a second runtime.
pub fn mcp_bridge(endpoint: PathBuf) -> Result<()> {
    mcp_transport(None, Some(endpoint))
}

fn mcp_transport(runtime: Option<Runtime>, endpoint: Option<PathBuf>) -> Result<()> {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    let output = Arc::new(Mutex::new(std::io::stdout()));
    let active = Arc::new(AtomicUsize::new(0));
    let pending = Arc::new(Mutex::new(std::collections::BTreeMap::<
        String,
        (String, String),
    >::new()));
    let mut handlers = Vec::<std::thread::JoinHandle<()>>::new();
    let write = |value: Value| -> Result<()> {
        let mut output = output.lock().unwrap();
        serde_json::to_writer(&mut *output, &value)?;
        output.write_all(b"\n")?;
        output.flush()?;
        Ok(())
    };
    while let Some(line) = read_frame(&mut std::io::stdin().lock())? {
        let mut req: Value = match serde_json::from_str(&line) {
            Ok(req) => req,
            Err(e) => {
                write(
                    json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":e.to_string()}}),
                )?;
                continue;
            }
        };
        if req["method"] == "notifications/cancelled" {
            let found = pending
                .lock()
                .unwrap()
                .get(&req["params"]["requestId"].to_string())
                .cloned();
            if let Some((instance, request_id)) = found {
                let command = json!({"op":"cancel","instance":instance,"request_id":request_id});
                if let Some(path) = &endpoint {
                    let _ = request(path, command);
                } else {
                    let _ = runtime.as_ref().unwrap().dispatch(command);
                }
            }
            continue;
        }
        if req.get("id").is_none() {
            continue;
        }
        handlers.retain(|h| !h.is_finished());
        if active.load(Ordering::Acquire) >= 16 {
            write(
                json!({"jsonrpc":"2.0","id":req["id"],"error":{"code":-32000,"message":"16 MCP requests already pending"}}),
            )?;
            continue;
        }
        let key = req["id"].to_string();
        if pending.lock().unwrap().contains_key(&key) {
            write(
                json!({"jsonrpc":"2.0","id":req["id"],"error":{"code":-32600,"message":"duplicate active request id"}}),
            )?;
            continue;
        }
        let command = req
            .pointer("/params/arguments")
            .cloned()
            .unwrap_or(Value::Null);
        let instance = command["instance"].as_str().unwrap_or("default").to_owned();
        let request_id = command["request_id"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("mcp:{}", uuid::Uuid::new_v4()));
        if let Some(command) = req
            .pointer_mut("/params/arguments")
            .filter(|v| v.is_object())
        {
            command["request_id"] = json!(request_id);
        }
        pending
            .lock()
            .unwrap()
            .insert(key.clone(), (instance, request_id));
        active.fetch_add(1, Ordering::AcqRel);
        let (runtime, endpoint, output, active, pending) = (
            runtime.clone(),
            endpoint.clone(),
            output.clone(),
            active.clone(),
            pending.clone(),
        );
        handlers.push(std::thread::spawn(move || {
            let response = mcp(req, &mut |command| {
                if let Some(path) = &endpoint {
                    request(path, command)
                } else {
                    Ok(envelope(runtime.as_ref().unwrap(), command))
                }
            });
            if let Some(response) = response {
                let mut output = output.lock().unwrap();
                let _ = serde_json::to_writer(&mut *output, &response);
                let _ = output.write_all(b"\n");
                let _ = output.flush();
            }
            pending.lock().unwrap().remove(&key);
            active.fetch_sub(1, Ordering::AcqRel);
        }));
    }
    // EOF means the client is gone; cancel its in-flight work before joining.
    let commands = pending
        .lock()
        .unwrap()
        .values()
        .map(|(instance, id)| json!({"op":"cancel","instance":instance,"request_id":id}))
        .collect::<Vec<_>>();
    for command in commands {
        if let Some(path) = &endpoint {
            let _ = request(path, command);
        } else {
            let _ = runtime.as_ref().unwrap().dispatch(command);
        }
    }
    for handler in handlers {
        let _ = handler.join();
    }
    Ok(())
}

pub fn serve(
    runtime: Runtime,
    endpoint_path: &Path,
    root: PathBuf,
    startup: Option<Value>,
) -> Result<()> {
    if endpoint_path.exists() {
        bail!(
            "endpoint file exists; use it to connect, or remove it only after verifying its PID is no longer running"
        );
    }
    let parent = endpoint_path
        .parent()
        .ok_or_else(|| anyhow!("endpoint requires a parent directory"))?;
    let fresh = !parent.exists();
    std::fs::create_dir_all(parent)?;
    restrict_directory(parent, fresh)?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let endpoint = Endpoint {
        address: listener.local_addr()?,
        token: format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ),
        pid: std::process::id(),
        root,
    };
    // Publish complete JSON atomically. A client watching for file creation must
    // never observe a half-written token/address. No-clobber also arbitrates
    // concurrent daemon startups before either can execute a startup script.
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(file.as_file_mut(), &endpoint)?;
    file.as_file().sync_all()?;
    file.persist_noclobber(endpoint_path).map_err(|e| e.error)?;
    struct RemoveOnDrop(PathBuf);
    impl Drop for RemoveOnDrop {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _guard = RemoveOnDrop(endpoint_path.to_owned());
    runtime.dispatch(json!({"op":"plugins_restore"}))?;
    // Reserve the endpoint before executing anything that can access hardware.
    // A second startup must fail without opening or querying a device.
    if let Some(startup) = startup {
        runtime.dispatch(startup)?;
    }
    eprintln!(
        "Listening on {}; endpoint {}",
        endpoint.address,
        endpoint_path.display()
    );
    let shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let active = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut handlers = Vec::new();
    let address = endpoint.address;
    for connection in listener.incoming() {
        let mut stream = connection?;
        if shutdown.load(std::sync::atomic::Ordering::Acquire) {
            break;
        }
        handlers.retain(|h: &std::thread::JoinHandle<()>| !h.is_finished());
        if active.load(std::sync::atomic::Ordering::Acquire) >= 32 {
            let _ = stream.write_all(
                b"{\"ok\":false,\"error\":{\"code\":\"busy\",\"message\":\"connection limit\"}}\n",
            );
            continue;
        }
        active.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let (rt, token, stop, count) = (
            runtime.clone(),
            endpoint.token.clone(),
            shutdown.clone(),
            active.clone(),
        );
        handlers.push(std::thread::spawn(move || {
            struct Guard(std::sync::Arc<std::sync::atomic::AtomicUsize>);
            impl Drop for Guard {
                fn drop(&mut self) {
                    self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
                }
            }
            let _guard = Guard(count);
            let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
            let incoming = read_connection_frame(&mut stream)
                .and_then(|line| Ok(serde_json::from_str::<Value>(&line)?));
            let mut terminate = false;
            let response = match incoming {
                Err(e) => json!({"ok":false,"error":crate::error::json_error(&e)}),
                Ok(req) if req["token"].as_str() != Some(&token) => {
                    json!({"ok":false,"error":{"code":"unauthorized","message":"unauthorized"}})
                }
                Ok(req) => {
                    let command = req["command"].clone();
                    if command["op"] == "shutdown" {
                        terminate = true;
                        json!({"id":command["id"],"ok":true,"result":{"shutdown":true}})
                    } else if command["op"] == "event_stream" {
                        let mut cursor = command["after"].as_u64().unwrap_or(0);
                        loop {
                            if stop.load(std::sync::atomic::Ordering::Acquire) {
                                break;
                            }
                            match rt.dispatch(json!({"op":"events","after":cursor,"wait_ms":1000}))
                            {
                                Ok(page) => {
                                    cursor = page["cursor"].as_u64().unwrap_or(cursor);
                                    if serde_json::to_writer(
                                        &mut stream,
                                        &json!({"ok":true,"result":page}),
                                    )
                                    .is_err()
                                        || stream.write_all(b"\n").is_err()
                                    {
                                        break;
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                        return;
                    } else {
                        envelope(&rt, command)
                    }
                }
            };
            let _ = serde_json::to_writer(&mut stream, &response);
            let _ = stream.write_all(b"\n");
            if terminate {
                stop.store(true, std::sync::atomic::Ordering::Release);
                rt.shared.changed.notify_all();
                let _ = TcpStream::connect_timeout(&address, Duration::from_millis(200));
            }
        }));
    }
    // Cancel Lua work before joining connection threads waiting for that work.
    if let Ok(names) = runtime.dispatch(json!({"op":"instances"}))
        && let Some(names) = names.as_array()
    {
        for name in names {
            let _ = runtime.dispatch(json!({"op":"destroy","instance":name}));
        }
    }
    for handler in handlers {
        let _ = handler.join();
    }

    Ok(())
}

fn restrict_directory(path: &Path, fresh: bool) -> Result<()> {
    #[cfg(windows)]
    {
        // Only newly created directories are modified. Existing directories must
        // already be private; never rewrite the ACL of an arbitrary user folder.
        let script = if fresh {
            r#"$ErrorActionPreference='Stop'; $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User; $acl=New-Object System.Security.AccessControl.DirectorySecurity; $acl.SetAccessRuleProtection($true,$false); $rule=New-Object System.Security.AccessControl.FileSystemAccessRule($sid,'FullControl','ContainerInherit,ObjectInherit','None','Allow'); $acl.AddAccessRule($rule); [System.IO.Directory]::SetAccessControl($env:PERIPHERAL_ENDPOINT_DIR,$acl)"#
        } else {
            r#"$ErrorActionPreference='Stop'; $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User; $acl=[System.IO.Directory]::GetAccessControl($env:PERIPHERAL_ENDPOINT_DIR); if (-not $acl.AreAccessRulesProtected) {throw 'Existing endpoint directory must have a private ACL; choose a new dedicated directory'}; foreach ($rule in $acl.GetAccessRules($true,$true,[System.Security.Principal.SecurityIdentifier])) { if ($rule.AccessControlType -eq 'Allow' -and $rule.IdentityReference -ne $sid) {throw 'Existing endpoint directory permits another identity; choose a new dedicated directory'} }"#
        };
        let status = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("PERIPHERAL_ENDPOINT_DIR", path)
            .output()?;
        if !status.status.success() {
            bail!(
                "could not restrict endpoint directory ACL: {}",
                String::from_utf8_lossy(&status.stderr)
            );
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fresh {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
        } else if std::fs::metadata(path)?.permissions().mode() & 0o077 != 0 {
            bail!("existing endpoint directory must be private (0700)");
        }
    }
    Ok(())
}

pub fn mcp(request: Value, dispatch: &mut impl FnMut(Value) -> Result<Value>) -> Option<Value> {
    if !request.is_object()
        || request["jsonrpc"] != "2.0"
        || !request["method"].is_string()
        || request
            .get("id")
            .is_some_and(|id| !(id.is_number() || id.as_str().is_some_and(|s| s.len() <= 128)))
    {
        return Some(
            json!({"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"invalid JSON-RPC request"}}),
        );
    }
    let id = request.get("id").cloned();
    let method = request["method"].as_str().unwrap_or("");
    let id = id?; // Notifications never produce stdout replies.
    let result: std::result::Result<Value, (i32, String)> = match method {
        "initialize" => {
            let requested = request["params"]["protocolVersion"]
                .as_str()
                .unwrap_or("2024-11-05");
            let version = if ["2024-11-05", "2025-03-26", "2025-06-18"].contains(&requested) {
                requested
            } else {
                "2025-06-18"
            };
            Ok(
                json!({"protocolVersion":version,"capabilities":{"tools":{},"resources":{}},"serverInfo":{"name":"peripheral-repl","version":env!("CARGO_PKG_VERSION")},"instructions":"First read peripheral_docs (language zh-CN or en), then peripheral_command ping and plugins. Use the existing backend; do not open a second owner of an active device. The handbook contains executable Lua, plugin installation, UI and debugging workflows. Unknown hardware reports must be tested with mock replay, never sent speculatively. Feature Reports require exact device and operation policies."}),
            )
        }
        "ping" => Ok(json!({})),
        "resources/list" => Ok(crate::guide::resources()),
        "resources/templates/list" => Ok(json!({"resourceTemplates":[]})),
        "resources/read" => crate::guide::read(request["params"]["uri"].as_str().unwrap_or(""))
            .ok_or((-32002, "unknown documentation resource".into())),
        "tools/list" => Ok(
            json!({"tools":[{"name":"peripheral_command","description":"Control shared Lua instances, BLE adapters/scanning, asynchronous tasks, configuration, typed actions and cursor events. Lua: ble.adapters/scan/list/open; runtime.spawn/sleep/join/cancel; gatt.services/read/subscribe. Calls may wait for asynchronous IO; timeout_ms is wall deadline, cpu_ms is active execution budget. HID Feature reads/writes and explicit control output are supported with exact host policies. Use channel.transaction for multi-step exchanges; device.watch/resolve/reconnect for HID lifecycle. Windows GATT supports attribute handles, cache modes, mtu and write_reliable; pairing requires an explicit pairing policy.","inputSchema":{"type":"object","properties":{
                "op":{"type":"string","enum":["plugins","plugin_enable","plugin_restart","plugin_configure","plugin_install","plugins_restore","scripts","script_read","script_import","ui_environment","ui_snapshot","ui_event","ui_dismiss","device_snapshot","config_description","reset","ping","devices","bluetooth","instances","create","eval","load","reload","tasks","stats","pause","resume","stop","destroy","snapshot","events","cancel","actions","invoke","configure","config","start","lifecycle"]},
                "source":{"type":"string"},"surface":{"type":"string"},"node":{"type":["string","null"]},"button":{"type":"string","enum":["left","right"]},"revision":{"type":"integer"},"schema":{"type":"object"},"plugin":{"type":"string"},"enabled":{"type":"boolean"},"manifest":{"type":"object"},"files":{"type":"object"},"generation":{"type":"string"},"instance":{"type":"string"},"code":{"type":"string"},"path":{"type":"string"},"timeout_ms":{"type":"integer","minimum":1,"maximum":120000},"cpu_ms":{"type":"integer","minimum":1,"maximum":2000},"request_id":{"type":"string"},"task":{"type":"integer"},"after":{"type":"integer"},"wait_ms":{"type":"integer","maximum":30000},"filter":{"type":"object"},"action":{"type":"string"},"duration_ms":{"type":"integer"},"adapter":{"type":"integer"},"name":{"type":"string"},"arguments":{"type":"object"},"value":{"type":"object"},"config":{"type":"object"},"metadata":{"type":"boolean"}},"required":["op"],"additionalProperties":false}},{"name":"peripheral_docs","description":"Read the bundled human/AI handbook before writing plugins or debugging devices. No backend or hardware connection needed.","annotations":{"readOnlyHint":true,"openWorldHint":false},"inputSchema":{"type":"object","properties":{"language":{"type":"string","enum":["zh-CN","en"]}},"required":["language"],"additionalProperties":false}}]}),
        ),
        "tools/call" if request["params"]["name"] == "peripheral_docs" => {
            match crate::guide::get(
                request["params"]["arguments"]["language"]
                    .as_str()
                    .unwrap_or(""),
            ) {
                Some(g) => Ok(json!({"content":[{"type":"text","text":g.text}],"isError":false})),
                None => Err((-32602, "language must be zh-CN or en".into())),
            }
        }
        "tools/call" if request["params"]["name"] == "peripheral_command" => {
            match dispatch(request["params"]["arguments"].clone()) {
                Ok(response) => {
                    let is_error = response["ok"] != true;
                    Ok(
                        json!({"content":[{"type":"text","text":response.to_string()}],"isError":is_error}),
                    )
                }
                Err(e) => {
                    Ok(json!({"content":[{"type":"text","text":e.to_string()}],"isError":true}))
                }
            }
        }
        "tools/call" => Err((-32602, "unknown tool".into())),
        _ => Err((-32601, "method not found".into())),
    };
    Some(match result {
        Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
        Err((code, message)) => {
            json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
        }
    })
}
