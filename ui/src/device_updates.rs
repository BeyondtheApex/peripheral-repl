//! Expiry is a one-shot deadline, not a periodic repaint or device poll.
use crate::{app::Application, control::Request};
use gpui_kit::*;
use serde_json::{Value, json};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
impl Application {
    pub fn schedule_device_expiry(&mut self, cx: &mut App) {
        self.device_expiry.clear();
        let Some(manager) = self
            .surfaces
            .get("main")
            .and_then(|s| s.view.read(cx).manager.as_ref())
        else {
            return;
        };
        if manager.backend["connection"] != "connected" {
            return;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        for plugin in manager.backend["plugins"].as_array().into_iter().flatten() {
            let d = &plugin["device"];
            if plugin["enabled"] != true
                || plugin["running"] != true
                || d.is_null()
                || d["stale"] == true
            {
                continue;
            }
            let deadline = d["updated_unix_ms"]
                .as_u64()
                .unwrap_or(0)
                .saturating_add(d["state"]["ttl_ms"].as_u64().unwrap_or(30000));
            let body = json!({"op":"device_expired","plugin":plugin["id"],"generation":d["generation"],"updated":d["updated_unix_ms"]});
            let commands = self.commands.clone();
            let task = cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(deadline.saturating_sub(now) + 1))
                    .await;
                let (tx, _) = std::sync::mpsc::sync_channel(1);
                let _ = commands
                    .send(Request {
                        body,
                        reply: tx,
                        expires: None,
                    })
                    .await;
            });
            self.device_expiry
                .insert(plugin["id"].as_str().unwrap_or("").to_owned(), task);
        }
    }
    pub fn expire_device(&mut self, event: &Value, cx: &mut App) {
        self.manager(cx, |m, _| {
            for p in m.backend["plugins"].as_array_mut().into_iter().flatten() {
                let matches = p["id"] == event["plugin"];
                let d = &mut p["device"];
                if matches
                    && d["generation"] == event["generation"]
                    && d["updated_unix_ms"] == event["updated"]
                {
                    d["stale"] = json!(true);
                }
            }
        });
    }
}
