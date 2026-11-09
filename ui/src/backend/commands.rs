use super::*;

pub(super) fn spawn_worker(
    rx: mpsc::Receiver<Value>,
    ep: PathBuf,
    notify: async_channel::Sender<Request>,
    root: Arc<Mutex<PathBuf>>,
) {
    std::thread::spawn(move || {
        for command in rx {
            let operation = command["op"].as_str().unwrap_or("");
            let result = (|| -> Result<Value> {
                match operation {
                    "connect" | "refresh" => refresh(&ep),
                    "workspace" => {
                        let path =
                            PathBuf::from(command["path"].as_str().context("workspace path")?);
                        if let Ok(ping) = request(&ep, json!({"op":"ping"})) {
                            let active =
                                PathBuf::from(ping["root"].as_str().context("active workspace")?)
                                    .canonicalize()?;
                            anyhow::ensure!(
                                path.canonicalize()? == active,
                                "Stop the backend before changing its workspace"
                            );
                        }
                        save_workspace(&path)?;
                        *root.lock().unwrap() = path.clone();
                        Ok(json!({"workspace":path}))
                    }
                    "start_backend" => {
                        lifecycle::start(&ep, &root.lock().unwrap())?;
                        refresh(&ep)
                    }
                    "stop_backend" => {
                        lifecycle::stop(&ep)?;
                        Ok(json!({"stopped":true}))
                    }
                    "restart_backend" => {
                        let ping = request(&ep, json!({"op":"ping"}))?;
                        let active =
                            PathBuf::from(ping["root"].as_str().context("backend workspace")?);
                        lifecycle::stop(&ep)?;
                        lifecycle::start(&ep, &active)?;
                        *root.lock().unwrap() = active;
                        refresh(&ep)
                    }
                    "install_plugin" => {
                        let path = Path::new(command["path"].as_str().context("plugin folder")?);
                        let body = json!({"op":"plugin_install","source":path.canonicalize()?});
                        request(&ep, body)
                    }
                    _ => request(&ep, command.clone()),
                }
            })();
            send(
                &notify,
                json!({"op":"backend_result","command":command,"ok":result.is_ok(),"result":result.as_ref().ok(),"error":result.as_ref().err().map(|e|format!("{e:#}"))}),
            );
            if ![
                "connect",
                "refresh",
                "workspace",
                "start_backend",
                "stop_backend",
                "restart_backend",
                "ui_snapshot",
                "eval",
                "invoke",
                "ui_event",
                "ui_dismiss",
                "shutdown",
            ]
            .contains(&operation)
                && let Ok(state) = refresh(&ep)
            {
                send(&notify, json!({"op":"backend_state","state":state}));
            }
        }
    });
}
