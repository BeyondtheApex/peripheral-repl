use crate::app::{Application, send};
use anyhow::{Context, Result, ensure};
use gpui_kit::*;
use serde_json::{Value, json};

impl Application {
    pub fn ai_command(&mut self, v: &Value, cx: &mut App) -> Option<Result<Value>> {
        let op = v["op"].as_str()?;
        if ![
            "ai_setup",
            "ai_format",
            "ai_copy_config",
            "ai_copy_prompt",
            "guide_open",
            "guide_close",
            "guide_copy",
            "ai_check",
            "ai_check_result",
        ]
        .contains(&op)
        {
            return None;
        }
        Some((|| {
            let manager = self.surfaces["main"]
                .view
                .read(cx)
                .manager
                .as_ref()
                .context("manager")?;
            let format = manager.ai_format.clone();
            let workspace = manager.workspace.clone();
            let language = v["language"].as_str().unwrap_or("zh-CN");
            match op {
                "ai_setup" => {
                    return Ok(json!({
                        "format":format,"configuration":super::configuration(&format,&std::env::current_exe()?,&self.backend.endpoint)?,
                        "checking":manager.ai_checking,"result":manager.ai_result,"document":manager.guide,
                        "documents":peripheral_repl::guide::resources()["resources"],"prompt":super::prompt(&workspace,language)
                    }));
                }
                "ai_format" => {
                    let format = v["format"].as_str().context("format")?;
                    ensure!(["json", "codex"].contains(&format), "unknown client format");
                    self.manager(cx, |m, _| m.ai_format = format.into());
                }
                "ai_copy_config" => {
                    let text = super::configuration(
                        &format,
                        &std::env::current_exe()?,
                        &self.backend.endpoint,
                    )?;
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                    self.status("已复制 MCP 配置；请粘贴到 AI 客户端并重新加载。".into(), cx);
                }
                "ai_copy_prompt" => {
                    ensure!(
                        peripheral_repl::guide::get(language).is_some(),
                        "unknown language"
                    );
                    cx.write_to_clipboard(ClipboardItem::new_string(super::prompt(
                        &workspace, language,
                    )));
                    self.status(
                        "已复制任务提示词；补充你的设备、功能和显示位置后发送给 AI。".into(),
                        cx,
                    );
                }
                "guide_open" => {
                    let guide =
                        peripheral_repl::guide::get(language).context("unknown document")?;
                    self.manager(cx, |m, _| {
                        m.transition();
                        m.page = "repl".into();
                        m.guide = Some(guide.language.into());
                    });
                }
                "guide_close" => self.manager(cx, |m, _| {
                    m.guide = None;
                    m.transition();
                }),
                "guide_copy" => {
                    let guide =
                        peripheral_repl::guide::get(language).context("unknown document")?;
                    cx.write_to_clipboard(ClipboardItem::new_string(format!(
                        "{}\n\n{}",
                        super::prompt(&workspace, language),
                        guide.text
                    )));
                    self.status(
                        "已复制完整手册和当前工作区说明，可直接发送给 AI。".into(),
                        cx,
                    );
                }
                "ai_check" => {
                    if manager.ai_checking {
                        return Ok(json!({"queued":false}));
                    }
                    self.manager(cx, |m, _| {
                        m.ai_checking = true;
                        m.ai_result = "正在检测 EXE → MCP → 当前后台…".into();
                    });
                    let endpoint = self.backend.endpoint.clone();
                    let sender = self.commands.clone();
                    std::thread::spawn(move || {
                        let result = super::probe::check(&endpoint);
                        send(
                            &sender,
                            json!({"op":"ai_check_result","result":result.as_ref().ok(),"error":result.err().map(|e|format!("{e:#}"))}),
                        );
                    });
                }
                "ai_check_result" => self.manager(cx, |m, _| {
                    m.ai_checking = false;
                    m.ai_result = if let Some(error) = v["error"].as_str() {
                        format!("连接失败：{error}")
                    } else if m.backend["ping"]["runtime_id"]
                        != v["result"]["backend"]["runtime_id"]
                    {
                        "MCP 已连通，但后台身份与界面不同；请重新连接后台后再检测。".into()
                    } else {
                        "检测通过 · MCP 工具可用，已连接本界面使用的后台。".into()
                    };
                }),
                _ => unreachable!(),
            }
            Ok(json!({"ok":true}))
        })())
    }
}
