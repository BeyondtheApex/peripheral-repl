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
                "EXE: {}",
                self.shell["executable"].as_str().unwrap_or("")
            )))
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
            .child(div().flex().items_center().gap_2()
                .child(label("任务栏位置"))
                .children([("left", "左侧"), ("right", "右侧")].into_iter().map(|(side,title)| {
                    button(format!("taskbar-side-{side}"), title,
                        if self.shell["side"].as_str().unwrap_or("right")==side { "circle-check" } else { "circle" },
                        json!({"op":"taskbar_side","side":side}),sender)
                })))
            .child(label("左侧：居中时贴着小组件右边；左对齐时自动使用右侧。数值越大越靠外，空间不足先隐藏中间的低优先级项目。"))
            .child(label("仅显示在主显示器；切换主屏或连接、断开显示器后会自动重新挂载。"))
            .child(label(match self.shell["layout"]["reason"].as_str() {
                Some("layout_unavailable") => "无法确认系统控件边界，已隐藏任务栏内容".into(),
                Some("insufficient_space") => "任务栏空间不足，已隐藏全部项目".into(),
                Some("ready") => crate::i18n::tr(&format!("任务栏显示 {} / {} 项", self.shell["layout"]["visible_items"],self.shell["layout"]["total_items"])),
                _ => String::new(),
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
            .children((pending && self.shell["update_result"]["state"] == "applied").then(|| label(
                "此前的成功记录不代表当前更新已完成；当前仍有待安装包。"
            )))
            .child(label(
                self.shell["update_result"]["message"]
                    .as_str()
                    .unwrap_or("")
                    .to_owned(),
            ));

        shell.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::Manager;
    use crate::control::Request;
    use gpui_core::{
        self as gpui, Context, IntoElement, Modifiers, Render, TestAppContext, VisualTestContext,
        Window, px, size,
    };
    use serde_json::json;

    #[gpui::test]
    fn taskbar_side_choices_dispatch_independently(cx: &mut TestAppContext) {
        struct Fixture {
            manager: Manager,
            sender: async_channel::Sender<Request>,
        }
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                self.manager.shell(&self.sender)
            }
        }
        cx.update(gpui_kit::init);
        let (sender, receiver) = async_channel::bounded(8);
        let window = cx.add_window(|window, cx| Fixture {
            manager: Manager::new(window, cx),
            sender,
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.simulate_resize(size(px(640.), px(700.)));
        visual.update(|w, cx| {
            w.draw(cx).clear(cx);
        });
        for (side, selector) in [
            ("left", "taskbar-side-left"),
            ("right", "taskbar-side-right"),
        ] {
            let bounds = visual.debug_bounds(selector).expect("side button");
            assert!(bounds.right() <= px(640.) && bounds.bottom() <= px(700.));
            visual.simulate_click(bounds.center(), Modifiers::default());
            assert_eq!(
                receiver.try_recv().unwrap().body,
                json!({"op":"taskbar_side","side":side})
            );
            assert!(receiver.try_recv().is_err());
        }
    }
}
