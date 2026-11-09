use crate::{backend, control, manager::Manager, surfaces::Surface};
use anyhow::Result;
use gpui_kit::*;
use peripheral_repl::ui::Publication;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(crate) struct Application {
    pub surfaces: BTreeMap<String, Surface>,
    pub commands: async_channel::Sender<control::Request>,
    pub backend: backend::Client,
    pub publications: Vec<Publication>,
    pub revision: Option<u64>,
    pub runtime_identity: String,
    pub taskbar_enabled: bool,
    pub shell: Value,
    pub explorer: Option<u32>,
    pub backend_status: crate::backend_status::BackendStatus,
    pub dashboard_options: crate::dashboard::Options,
    pub device_expiry: BTreeMap<String, Task<()>>,
    pub initial_main_hidden: bool,
}
impl Application {
    pub fn new(commands: async_channel::Sender<control::Request>, start_hidden: bool) -> Self {
        let backend = backend::Client::new(commands.clone());
        let enabled = std::fs::read(backend::data_dir().join("preferences.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .is_some_and(|v| v["taskbar_enabled"] == true);
        Self {
            surfaces: BTreeMap::new(),
            commands,
            backend,
            publications: Vec::new(),
            revision: None,
            runtime_identity: String::new(),
            taskbar_enabled: enabled,
            shell: json!({}),
            explorer: None,
            backend_status: Default::default(),
            dashboard_options: crate::dashboard::Options::load(),
            device_expiry: BTreeMap::new(),
            initial_main_hidden: start_hidden,
        }
    }
    pub fn manager(&mut self, cx: &mut App, update: impl FnOnce(&mut Manager, &mut App)) {
        if let Some(main) = self.surfaces.get("main") {
            main.view.update(cx, |view, cx| {
                if let Some(manager) = &mut view.manager {
                    update(manager, cx);
                    cx.notify();
                }
            });
        }
    }
    pub fn status(&mut self, message: String, cx: &mut App) {
        self.manager(cx, |m, _| {
            m.status = message;
        });
    }
    pub fn command(&mut self, v: &Value, cx: &mut App) -> Result<Value> {
        if let Some(result) = self.startup_command(v, cx) {
            return result;
        }
        if let Some(result) = self.clipboard_command(v, cx) {
            return result;
        }
        if let Some(result) = self.app_menu_command(v, cx) {
            return result;
        }
        if let Some(result) = self.appearance_command(v, cx) {
            return result;
        }
        if let Some(result) = self.ai_command(v, cx) {
            return result;
        }
        if v["op"] == "device_expired" {
            self.expire_device(v, cx);
            return Ok(json!({"ok":true}));
        }
        if let Some(result) = self.dashboard_command(v, cx) {
            return result;
        }
        if v["op"] == "backend_control" {
            self.control_backend(v["action"].as_str().unwrap_or(""), cx)?;
            return Ok(json!({"queued":true}));
        }
        if let Some(result) = self.plugin_command(v, cx) {
            return result;
        }
        if let Some(result) = self.management_command(v, cx) {
            return result;
        }
        if let Some(result) = self.shell_command(v, cx) {
            return result;
        }
        self.surface_command(v, cx)
    }
}
pub(crate) fn send(sender: &async_channel::Sender<control::Request>, body: Value) {
    let (tx, _) = std::sync::mpsc::sync_channel(1);
    let _ = sender.send_blocking(control::Request {
        body,
        reply: tx,
        expires: None,
    });
}
