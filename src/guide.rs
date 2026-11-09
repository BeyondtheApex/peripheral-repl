//! Versioned, offline handbooks shared by the UI and MCP; never read arbitrary files.
use serde_json::{Value, json};

pub struct Guide {
    pub language: &'static str,
    pub title: &'static str,
    pub uri: &'static str,
    pub text: &'static str,
}
pub const GUIDES: [Guide; 2] = [
    Guide {
        language: "zh-CN",
        title: "插件开发与设备调试 · 中文",
        uri: "peripheral://docs/zh-CN",
        text: include_str!("handbooks/zh-CN.md"),
    },
    Guide {
        language: "en",
        title: "Plugin development & device debugging · English",
        uri: "peripheral://docs/en",
        text: include_str!("handbooks/en.md"),
    },
];
pub fn get(language: &str) -> Option<&'static Guide> {
    GUIDES.iter().find(|g| g.language == language)
}
pub fn resources() -> Value {
    json!({"resources":GUIDES.iter().map(|g| json!({"uri":g.uri,"name":g.language,"title":g.title,"mimeType":"text/markdown","description":"Offline Lua API, MCP/REPL, plugin authoring, UI and evidence-based device debugging handbook"})).collect::<Vec<_>>()})
}
pub fn read(uri: &str) -> Option<Value> {
    GUIDES
        .iter()
        .find(|g| g.uri == uri)
        .map(|g| json!({"contents":[{"uri":g.uri,"mimeType":"text/markdown","text":g.text}]}))
}
