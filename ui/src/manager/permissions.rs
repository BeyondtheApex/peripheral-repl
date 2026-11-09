//! Host-rendered permission review, independent of plugin code and surfaces.
use super::*;
pub struct Editor {
    pub id: String,
    pub name: String,
    pub description: Value,
    pub selected: std::collections::BTreeSet<String>,
    pub expanded: std::collections::BTreeSet<String>,
    pub saving: bool,
}
impl Editor {
    pub fn new(plugin: &Value) -> Self {
        let description = plugin["permissions"].clone();
        let selected = description["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|item| item["approved"] == true)
            .filter_map(|item| item["request"]["id"].as_str().map(str::to_owned))
            .collect();
        Self {
            id: plugin["id"].as_str().unwrap_or("").into(),
            name: plugin["name"].as_str().unwrap_or("").into(),
            description,
            selected,
            expanded: Default::default(),
            saving: false,
        }
    }
    pub fn render(&self, sender: &async_channel::Sender<Request>) -> AnyElement {
        let mut panel=div().flex().flex_col().gap_4()
            .child(div().flex().items_center().justify_between().gap_3()
                .child(div().text_2xl().child(format!("{} · {}",self.name,crate::i18n::tr("设备权限"))))
                .child(button("permission-close","关闭","x",json!({"op":"permissions_close"}),sender)))
            .child(label("以下设备操作由插件申请。授权仅对这个插件生效，修改后会重新启动该插件；撤销必需权限将暂停运行。"));
        if let Some(error) = self.description["error"].as_str() {
            panel = panel.child(label(error.to_owned()));
        }
        let items = self.description["items"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if items.is_empty() {
            panel = panel.child(label(
                "此插件未声明需要授权的操作。若设备查询被拒绝，请让插件作者补充权限声明。",
            ));
        }
        for item in &items {
            let request = &item["request"];
            let id = request["id"].as_str().unwrap_or("");
            let expanded = self.expanded.contains(id);
            let mut card = div()
                .p_4()
                .rounded_lg()
                .bg(crate::appearance::panel("surface"))
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(icon("shield-check"))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(request["title"].as_str().unwrap_or(id).to_owned()),
                        )
                        .child(label(if request["required"] == true {
                            "必需"
                        } else {
                            "可选"
                        }))
                        .child(toggle(
                            format!("permission-{id}"),
                            self.selected.contains(id),
                            json!({"op":"permissions_toggle","id":id}),
                            sender,
                        )),
                )
                .child(label(
                    request["description"].as_str().unwrap_or("").to_owned(),
                ))
                .child(button(
                    format!("permission-details-{id}"),
                    if expanded {
                        "收起参数"
                    } else {
                        "查看设备与报文参数"
                    },
                    "list",
                    json!({"op":"permissions_details","id":id}),
                    sender,
                ));
            if expanded {
                card = card.child(parameters(&request["policy"]));
            }
            panel = panel.child(card);
        }
        let buttons = if self.saving {
            label("正在保存…").into_any_element()
        } else {
            div()
                .flex()
                .flex_wrap()
                .gap_3()
                .child(button(
                    "permission-save",
                    "保存授权选择",
                    "check",
                    json!({"op":"permissions_save"}),
                    sender,
                ))
                .children((!items.is_empty()).then(|| {
                    button(
                        "permission-allow-all",
                        "允许以上操作",
                        "shield-check",
                        json!({"op":"permissions_save","allow_all":true}),
                        sender,
                    )
                }))
                .into_any_element()
        };
        panel.child(buttons).into_any_element()
    }
}
fn parameters(policy: &Value) -> AnyElement {
    let mut panel = div().flex().flex_col().gap_3();
    for (key, operation) in [
        ("output_rules", "HID Output"),
        ("hid_rules", "HID Feature / Control"),
        ("ble_write_rules", "Bluetooth GATT"),
        ("ble_pair_rules", "Bluetooth Pairing"),
    ] {
        for rule in policy[key].as_array().into_iter().flatten() {
            let mut content = div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_sm().child(operation));
            // Canonical parameter order is stable across Rust/serde feature sets.
            for field in [
                "operation",
                "vendor_id",
                "product_id",
                "usage_page",
                "usage",
                "interface",
                "path",
                "service",
                "characteristic",
                "descriptor",
                "mode",
                "protection",
                "service_handle",
                "characteristic_handle",
                "descriptor_handle",
            ] {
                if let Some(value) = rule.get(field).filter(|v| !v.is_null()) {
                    let raw = value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string());
                    let raw = if ["vendor_id", "product_id", "usage_page", "usage"].contains(&field)
                    {
                        format!("0x{:04X}", value.as_u64().unwrap_or(0))
                    } else {
                        raw
                    };
                    content = content.child(label(format!("{field}: {raw}")));
                }
            }
            if let Some(packets) = rule["packets"].as_array() {
                content = content.child(label(format!(
                    "{}: {}",
                    crate::i18n::tr("精确报文"),
                    packets.len()
                )));
                for (index, packet) in packets.iter().enumerate() {
                    content =
                        content.child(div().text_xs().font_family("Consolas").child(format!(
                            "{}: {}",
                            index + 1,
                            packet.as_str().unwrap_or("")
                        )));
                }
            }
            panel = panel.child(
                div()
                    .id(SharedString::from(format!("{key}-{}", panel_id(rule))))
                    .overflow_x_scroll()
                    .p_3()
                    .rounded_lg()
                    .bg(crate::appearance::panel("elevated"))
                    .child(content),
            );
        }
    }
    panel.into_any_element()
}
fn panel_id(value: &Value) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    value.to_string().hash(&mut hash);
    hash.finish()
}
