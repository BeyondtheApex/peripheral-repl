use super::*;
impl Manager {
    pub(super) fn shell(&self, sender: &async_channel::Sender<Request>) -> AnyElement {
        let pending = self.shell["pending"].is_object();
        let shell = div()
            .flex()
            .flex_col()
            .gap_3()
            .p_5()
            .bg(crate::appearance::color(PANEL))
            .rounded_xl()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(div().text_lg().child(crate::i18n::tr("任务栏与更新")))
                    .child(label(
                        self.shell["state"]
                            .as_str()
                            .unwrap_or("正在检查…")
                            .to_owned(),
                    )),
            )
            .child(label(format!(
                "Explorer {} · {}",
                self.shell["explorer_pid"],
                self.shell["compatibility"].as_str().unwrap_or("")
            )))
            .child(label(format!(
                "DLL: {}",
                self.shell["dll"].as_str().unwrap_or("")
            )))
            .children(
                self.shell["modules"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|v| label(format!("Loaded: {}", v.as_str().unwrap_or("")))),
            )
            .child(label(if pending {
                "更新已准备，等待资源管理器释放旧版本"
            } else {
                "当前没有待安装的更新"
            }))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(div().flex().items_center().gap_3().child(label("任务栏内嵌显示")).child(toggle("taskbar-power",self.shell["enabled"]==true,json!({"op":"taskbar_enable","enabled":self.shell["enabled"]!=true}),sender)))
                    .child(button(
                        "stage-update",
                        "准备更新…",
                        "download",
                        json!({"op":"pick_update"}),
                        sender,
                    ))
                    .child(button(
                        "restart-explorer",
                        if pending {
                            "重启并更新…"
                        } else {
                            "重启资源管理器…"
                        },
                        "refresh-cw",
                        json!({"op":"manager_confirm","action":"restart_explorer"}),
                        sender,
                    )),
            )
            .child(label(
                self.shell["update_result"]["message"]
                    .as_str()
                    .unwrap_or("")
                    .to_owned(),
            ));

        shell.into_any_element()
    }
}
