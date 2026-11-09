//! Exercise the shipped stdio bridge off the UI thread without any hardware I/O.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    io::{BufReader, Write},
    os::windows::process::CommandExt,
    path::Path,
    process::{Command, Stdio},
    sync::mpsc,
    time::Duration,
};

pub fn check(endpoint: &Path) -> Result<Value> {
    let mut child = Command::new(std::env::current_exe()?)
        .arg("--mcp")
        .arg(endpoint)
        .creation_flags(0x08000000)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut input = child.stdin.take().context("MCP stdin")?;
    let output = child.stdout.take().context("MCP stdout")?;
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut reader = BufReader::new(output);
        loop {
            let result = peripheral_repl::protocol::read_frame(&mut reader)
                .and_then(|s| s.context("MCP bridge closed stdout"))
                .and_then(|s| Ok(serde_json::from_str::<Value>(&s)?));
            let failed = result.is_err();
            if tx.send(result).is_err() || failed {
                break;
            }
        }
    });
    let result = (|| {
        let mut call = |id: u64, method: &str, params: Value| -> Result<Value> {
            writeln!(
                input,
                "{}",
                json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
            )?;
            input.flush()?;
            let reply = rx
                .recv_timeout(Duration::from_secs(5))
                .context("MCP response timed out")??;
            ensure!(
                reply["id"] == id && reply["error"].is_null(),
                "MCP rejected request: {reply}"
            );
            if method == "initialize" {
                writeln!(
                    input,
                    "{}",
                    json!({"jsonrpc":"2.0","method":"notifications/initialized"})
                )?;
                input.flush()?;
            }
            Ok(reply["result"].clone())
        };
        let hello = call(
            1,
            "initialize",
            json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"peripheral-ui-check","version":"1"}}),
        )?;
        ensure!(
            hello["serverInfo"]["name"] == "peripheral-repl",
            "unexpected MCP server"
        );
        let tools = call(2, "tools/list", json!({}))?;
        for name in ["peripheral_command", "peripheral_docs"] {
            ensure!(
                tools["tools"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|t| t["name"] == name)),
                "missing MCP tool {name}"
            );
        }
        let ping = call(
            3,
            "tools/call",
            json!({"name":"peripheral_command","arguments":{"op":"ping"}}),
        )?;
        ensure!(
            ping["isError"] != true,
            "backend unavailable: {}",
            ping["content"]
        );
        let envelope: Value =
            serde_json::from_str(ping["content"][0]["text"].as_str().context("ping text")?)?;
        ensure!(envelope["ok"] == true, "backend ping failed");
        Ok(
            json!({"protocol":hello["protocolVersion"],"backend":envelope["result"],"hardware_io":false}),
        )
    })();
    drop(input);
    // This is only our disposable probe child, never the backend or an AI client.
    let _ = child.kill();
    let _ = child.wait();
    let _ = reader.join();
    result
}
