//! Shared copy completion and finite button feedback, independent of backend state.
use crate::{app::Application, control::Request, native};
use anyhow::{Context, Result};
use gpui_kit::*;
use serde_json::{Value, json};
use std::time::Duration;

impl Application {
    pub(crate) fn copy_text(&mut self, key: &str, text: String, cx: &mut App) -> Result<()> {
        let mut revision = 0;
        self.manager(cx, |m, _| {
            m.copy_revision = m.copy_revision.wrapping_add(1);
            revision = m.copy_revision;
            m.copied.remove(key);
        });
        self.try_copy_text(key, text, revision, 0, cx)
    }
    fn try_copy_text(
        &mut self,
        key: &str,
        text: String,
        revision: u64,
        attempt: u64,
        cx: &mut App,
    ) -> Result<()> {
        let current = self
            .surfaces
            .get("main")
            .and_then(|s| s.view.read(cx).manager.as_ref())
            .is_some_and(|m| m.copy_revision == revision);
        if !current {
            return Ok(());
        } // A newer click supersedes an old retry.
        let hwnd = self
            .surfaces
            .get("main")
            .context("main window unavailable")?
            .hwnd;
        if let Err(error) = native::clipboard::write_text(hwnd, &text) {
            if attempt >= 5 {
                return Err(error.context(crate::i18n::tr("复制失败，请重试")));
            }
            let commands = self.commands.clone();
            let key = key.to_owned();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_millis(40)).await;
                let (reply, _) = std::sync::mpsc::sync_channel(1);
                let _ = commands.send(Request { body: json!({"op":"clipboard_retry","key":key,"text":text,"revision":revision,"attempt":attempt+1}), reply, expires: None }).await;
            }).detach();
            return Ok(());
        }
        self.manager(cx, |m, _| {
            m.copied.insert(key.into(), revision);
            m.status = match key {
                "config" => "已复制 MCP 配置；请粘贴到 AI 客户端并重新加载。",
                "prompt" => "已复制任务提示词；补充你的设备、功能和显示位置后发送给 AI。",
                _ => "已复制完整手册和当前工作区说明，可直接发送给 AI。",
            }
            .into();
        });
        let commands = self.commands.clone();
        let key = key.to_owned();
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1600))
                .await;
            let (reply, _) = std::sync::mpsc::sync_channel(1);
            let _ = commands
                .send(Request {
                    body: json!({"op":"clipboard_feedback_clear","key":key,"revision":revision}),
                    reply,
                    expires: None,
                })
                .await;
        })
        .detach();
        Ok(())
    }
    pub(crate) fn clipboard_command(
        &mut self,
        value: &Value,
        cx: &mut App,
    ) -> Option<Result<Value>> {
        match value["op"].as_str()? {
            "clipboard_retry" => Some((|| {
                self.try_copy_text(
                    value["key"].as_str().context("copy key")?,
                    value["text"].as_str().context("copy text")?.into(),
                    value["revision"].as_u64().context("copy revision")?,
                    value["attempt"].as_u64().unwrap_or(5),
                    cx,
                )?;
                Ok(json!({"queued":true}))
            })()),
            "clipboard_feedback_clear" => {
                self.manager(cx, |m, _| {
                    if let Some(key) = value["key"].as_str()
                        && m.copied.get(key).copied() == value["revision"].as_u64()
                    {
                        m.copied.remove(key);
                    }
                });
                Some(Ok(json!({})))
            }
            _ => None,
        }
    }
}
