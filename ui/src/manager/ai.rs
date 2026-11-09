//! Compact onboarding and an offline, selectable Markdown reader.
use super::*;
use gpui_kit::base::text::TextView;

impl Manager {
    pub(super) fn ai_setup(&self, sender: &async_channel::Sender<Request>) -> AnyElement {
        let config = std::env::current_exe()
            .ok()
            .and_then(|exe| {
                crate::ai_setup::configuration(&self.ai_format, &exe, &crate::backend::endpoint())
                    .ok()
            })
            .unwrap_or_else(|| "无法取得程序路径".into());
        div().flex().flex_col().gap_3().p_4().rounded_xl().bg(crate::appearance::color(PANEL))
            .child(div().flex().items_center().gap_2().child(icon("bot"))
                .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(crate::i18n::tr("连接 AI")))
                .child(div().flex_1()).child(label("本机 MCP · 复用当前后台")))
            .child(label("① 启动后台　② 将配置粘贴到 AI 客户端并重新加载　③ 发送手册和你的需求"))
            .child(div().flex().flex_wrap().items_center().gap_2().children(
                [("json","通用 MCP JSON"),("codex","Codex TOML")].map(|(format,title)|{
                    button(format!("ai-format-{format}"),if self.ai_format==format {if format=="json"{"✓ 通用 MCP JSON"}else{"✓ Codex TOML"}}else{title},"code",
                        json!({"op":"ai_format","format":format}),sender)
                })
            ).child(div().flex_1()).child(button("ai-copy-config","复制配置","copy",json!({"op":"ai_copy_config"}),sender)))
            .child(div().id("ai-config-preview").h(px(112.)).overflow_y_scroll().p_3().rounded_lg().bg(crate::appearance::color(BG))
                .font_family("Cascadia Code").text_xs().children(config.lines().map(|line|div().child(line.to_owned()))))
            .child(div().flex().flex_wrap().items_center().gap_2()
                .child(button("ai-check",if self.ai_checking {"检测中…"}else{"检测 MCP"},"plug",json!({"op":"ai_check"}),sender))
                .child(button("ai-copy-prompt","复制任务提示词","message-square",json!({"op":"ai_copy_prompt"}),sender))
                .child(label(self.ai_result.clone())))
            .child(label("模型和登录在 AI 客户端配置；无需填写令牌。检测验证本机桥接，不代表客户端已加载配置。"))
            .into_any_element()
    }

    pub(super) fn guide_list(&self, sender: &async_channel::Sender<Request>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(crate::i18n::tr("开发文档")),
            )
            .child(label(
                "人和 AI 共用 · 离线内置 · Lua API / REPL / 插件 / 设备调试 / 多目标 UI",
            ))
            .children(peripheral_repl::guide::GUIDES.iter().map(|g| {
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .py_3()
                    .rounded_lg()
                    .bg(crate::appearance::color(PANEL))
                    .child(icon("book-open"))
                    .child(div().flex_1().child(g.title))
                    .child(button(
                        format!("guide-{}", g.language),
                        "阅读",
                        "arrow-up-right",
                        json!({"op":"guide_open","language":g.language}),
                        sender,
                    ))
                    .child(button(
                        format!("guide-copy-{}", g.language),
                        "复制给 AI",
                        "copy",
                        json!({"op":"guide_copy","language":g.language}),
                        sender,
                    ))
            }))
            .into_any_element()
    }

    pub(super) fn guide_reader(
        &self,
        language: &str,
        sender: &async_channel::Sender<Request>,
    ) -> AnyElement {
        let guide = peripheral_repl::guide::get(language).unwrap();
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(button(
                        "guide-back",
                        "返回控制台",
                        "arrow-left",
                        json!({"op":"guide_close"}),
                        sender,
                    ))
                    .child(div().flex_1())
                    .child(button(
                        "guide-copy",
                        "复制完整文档给 AI",
                        "copy",
                        json!({"op":"guide_copy","language":language}),
                        sender,
                    )),
            )
            .child(
                div()
                    .p_5()
                    .rounded_xl()
                    .bg(crate::appearance::color(PANEL))
                    .child(TextView::markdown(
                        SharedString::from(format!("guide-{language}")),
                        guide.text,
                    )),
            )
            .into_any_element()
    }
}
