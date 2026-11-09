//! Built-in session and script management. Each panel is independent.
use crate::control::Request;
use gpui_kit::{
    component::input::{Input, InputState},
    *,
};
use serde_json::{Value, json};
mod ai;
pub(crate) mod configuration;
mod configuration_fields;
mod dashboard_settings;
mod devices;
mod feedback;
mod flip;
pub(crate) mod motion;
mod navigation;
pub(crate) mod permissions;
mod plugin_details;
mod plugins;
mod repl;
mod settings;
mod shell;
mod titlebar;
pub(crate) mod widgets;
use widgets::*;
pub struct Manager {
    pub backend: Value,
    pub backend_status: crate::backend_status::BackendStatus,
    pub dashboard_options: crate::dashboard::Options,
    pub shell: Value,
    pub output: Vec<String>,
    pub selected: String,
    pub page: String,
    pub plugin_config: Option<configuration::Editor>,
    pub permission_editor: Option<permissions::Editor>,
    pub permission_prompted: std::collections::BTreeSet<String>,
    pub expanded_plugins: std::collections::BTreeSet<String>,
    pub status: String,
    pub code: Entity<InputState>,
    pub config: Entity<InputState>,
    pub confirm: Option<String>,
    pub workspace: String,
    pub ai_format: String,
    pub ai_checking: bool,
    pub ai_result: String,
    pub guide: Option<String>,
    pub copied: std::collections::BTreeMap<String, u64>,
    pub copy_revision: u64,
    pub device_faces: std::collections::BTreeMap<String, (bool, u64)>,
    pub page_revision: u64,
    pub startup: crate::startup::State,
    pub notice: Option<crate::notifications::Notice>,
    pub notice_revision: u64,
}
impl Manager {
    pub fn new(window: &mut Window, cx: &mut App) -> Self {
        Self {
            backend: json!({"connection":"disconnected"}),
            backend_status: Default::default(),
            dashboard_options: crate::dashboard::Options::load(),
            shell: json!({}),
            output: Vec::new(),
            selected: "default".into(),
            page: "plugins".into(),
            plugin_config: None,
            permission_editor: None,
            permission_prompted: Default::default(),
            expanded_plugins: Default::default(),
            status: "就绪".into(),
            code: cx.new(|cx| {
                InputState::new(window, cx).placeholder(crate::i18n::tr("输入 Lua 表达式或语句"))
            }),
            config: cx.new(|cx| {
                InputState::new(window, cx).placeholder(crate::i18n::tr("高级配置 JSON（可选）"))
            }),
            confirm: None,
            workspace: crate::backend::workspace().display().to_string(),
            ai_format: "json".into(),
            ai_checking: false,
            ai_result: "尚未检测 · 启动后台后可验证 MCP 连接。".into(),
            guide: None,
            copied: Default::default(),
            copy_revision: 0,
            device_faces: Default::default(),
            page_revision: 0,
            startup: crate::startup::state(),
            notice: None,
            notice_revision: 0,
        }
    }
    pub fn transition(&mut self) {
        self.page_revision = self.page_revision.wrapping_add(1);
    }
    pub fn log(&mut self, mut text: String) {
        if text.len() > 4096 {
            let mut end = 4096;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            text.push('…');
        }
        self.output.push(text);
        if self.output.len() > 100 {
            self.output.remove(0);
        }
    }
    pub fn render(&self, sender: &async_channel::Sender<Request>, _cx: &App) -> AnyElement {
        let content = match self.page.as_str() {
            "repl" => self.repl(sender),
            "settings" => self.settings(sender),
            "devices" => self.devices(sender),
            _ => self.plugins(sender),
        };
        let mut confirmation = None;
        if let Some(action) = &self.confirm {
            let message = if action == "restart_explorer" {
                "重启资源管理器后，任务栏和文件管理窗口可能暂时关闭。已准备的更新会一起替换 EXE 和 DLL。"
            } else if action == "shutdown_backend" {
                "关闭后台会停止所有插件并断开设备。插件的启用选项与配置会保留，下次启动后台时恢复。"
            } else {
                "这会清理当前调试会话及其视图。继续？"
            };
            confirmation = Some(
                div()
                    .p_5()
                    .rounded_xl()
                    .bg(crate::appearance::color(0x42352b))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(crate::i18n::tr(message))
                    .child(
                        div()
                            .flex()
                            .gap_3()
                            .child(button(
                                "confirm-action",
                                "确认",
                                "check",
                                json!({"op":"manager_confirmed","action":action}),
                                sender,
                            ))
                            .child(button(
                                "cancel-action",
                                "取消",
                                "x",
                                json!({"op":"manager_confirm","action":null}),
                                sender,
                            )),
                    )
                    .into_any_element(),
            );
        }
        div()
            .size_full()
            .flex()
            .bg(crate::appearance::background())
            .text_color(crate::appearance::color(TEXT))
            .font_family("Microsoft YaHei UI")
            .child(self.navigation(sender))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .children(
                        crate::appearance::options()
                            .hide_titlebar
                            .then(|| self.titlebar(sender)),
                    )
                    .children(confirmation)
                    .child(
                        div()
                            .id("management-scroll")
                            .flex_1()
                            .min_h_0()
                            .min_w_0()
                            .overflow_x_hidden()
                            .overflow_y_scroll()
                            .child(div().w_full().min_w_0().p_7().child(motion::enter(
                                format!("page-{}-{}", self.page, self.page_revision),
                                content,
                            ))),
                    )
                    .child(
                        div()
                            .px_6()
                            .h(px(32.))
                            .flex_shrink_0()
                            .truncate()
                            .py_2()
                            .text_xs()
                            .text_color(crate::appearance::color(MUTED))
                            .child(crate::i18n::tr(&self.status)),
                    ),
            )
            .into_any_element()
    }
}
