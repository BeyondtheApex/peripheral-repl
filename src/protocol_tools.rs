//! MCP schema stays in one place as the command API grows.
use serde_json::{Value, json};
pub fn list() -> Value {
    let mut properties = serde_json::Map::new();
    for (kind, names) in [
        (
            "string",
            vec![
                "source",
                "surface",
                "plugin",
                "generation",
                "instance",
                "code",
                "path",
                "request_id",
                "action",
                "name",
            ],
        ),
        (
            "object",
            vec![
                "policy",
                "schema",
                "manifest",
                "files",
                "filter",
                "arguments",
                "value",
                "config",
            ],
        ),
        ("boolean", vec!["enabled", "metadata"]),
        ("integer", vec!["task", "after", "duration_ms", "adapter"]),
    ] {
        for name in names {
            properties.insert(name.into(), json!({"type":kind}));
        }
    }
    properties.extend([
        ("op".into(),json!({"type":"string","enum":["plugins","plugin_enable","plugin_restart","plugin_configure","plugin_install","plugin_permissions","plugin_permissions_save","session_policy","plugins_restore","scripts","script_read","script_import","ui_environment","ui_snapshot","ui_event","ui_dismiss","device_snapshot","config_description","reset","ping","devices","bluetooth","instances","create","eval","load","reload","tasks","stats","pause","resume","stop","destroy","snapshot","events","cancel","actions","invoke","configure","config","start","lifecycle"]})),
        ("revision".into(),json!({"type":["integer","string"]})),
        ("node".into(),json!({"type":["string","null"]})),
        ("button".into(),json!({"type":"string","enum":["left","right"]})),
        ("approved".into(),json!({"type":"array","items":{"type":"string"}})),
        ("timeout_ms".into(),json!({"type":"integer","minimum":1,"maximum":120000})),
        ("cpu_ms".into(),json!({"type":"integer","minimum":1,"maximum":2000})),
        ("wait_ms".into(),json!({"type":"integer","maximum":30000})),
    ]);
    json!({"tools":[
        {"name":"peripheral_command","description":"Control the shared Lua runtime, plugins, device permissions, BLE/HID, configuration and UI. Read peripheral_docs first. Inspect plugin_permissions and ask the user before granting exact device operations with plugin_permissions_save. Unknown device reports must never be sent speculatively. session_policy inspects effective grants; create accepts a policy override for an independent debugging session.","inputSchema":{"type":"object","properties":properties,"required":["op"],"additionalProperties":false}},
        {"name":"peripheral_docs","description":"Read the bundled human/AI handbook before writing plugins or debugging devices. No backend or hardware connection needed.","annotations":{"readOnlyHint":true,"openWorldHint":false},"inputSchema":{"type":"object","properties":{"language":{"type":"string","enum":["zh-CN","en"]}},"required":["language"],"additionalProperties":false}}
    ]})
}
