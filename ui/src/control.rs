//! Local agent transport. Commands wake the UI executor; no polling render loop.
use anyhow::Result;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    sync::mpsc,
    time::Duration,
};

pub struct Request {
    pub body: Value,
    pub reply: mpsc::SyncSender<Value>,
    pub expires: Option<std::time::Instant>,
}

pub fn start() -> Result<(
    async_channel::Sender<Request>,
    async_channel::Receiver<Request>,
    std::path::PathBuf,
)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let token = uuid::Uuid::new_v4().to_string();
    let dir = crate::backend::data_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("session.json");
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({
            "address":listener.local_addr()?.to_string(), "token":token,
            "pid":std::process::id(), "protocol":1
        }))?,
    )?;
    let (sender, receiver) = async_channel::bounded(32);
    let ui_sender = sender.clone();
    std::thread::Builder::new()
        .name("ui-control".into())
        .spawn(move || {
            // Sequential bounded requests avoid unbounded client threads and queued frames.
            for stream in listener.incoming().flatten() {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                use std::io::Read;
                if reader.by_ref().take(65537).read_line(&mut line).is_err()
                    || line.len() > 65536
                    || !line.ends_with('\n')
                {
                    continue;
                }
                let reply = match serde_json::from_str::<Value>(&line) {
                    Ok(mut body) if body["token"].as_str() == Some(&token) => {
                        body.as_object_mut().unwrap().remove("token");
                        let (tx, rx) = mpsc::sync_channel(1);
                        match sender.try_send(Request {
                            body,
                            reply: tx,
                            expires: Some(std::time::Instant::now() + Duration::from_secs(4)),
                        }) {
                            Ok(()) => rx
                                .recv_timeout(Duration::from_secs(5))
                                .unwrap_or(json!({"ok":false,"error":"timeout"})),
                            Err(_) => json!({"ok":false,"error":"busy"}),
                        }
                    }
                    _ => json!({"ok":false,"error":"unauthorized_or_invalid"}),
                };
                let _ = writeln!(reader.get_mut(), "{}", reply);
            }
        })?;
    Ok((ui_sender, receiver, path))
}
