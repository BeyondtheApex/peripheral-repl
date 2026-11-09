//! AI connection configuration and bundled documentation, independent of rendering.
use anyhow::{Result, bail};
use serde_json::json;
use std::path::Path;
mod commands;
mod probe;

pub fn configuration(format: &str, exe: &Path, endpoint: &Path) -> Result<String> {
    let command = exe.to_string_lossy();
    let endpoint = endpoint.to_string_lossy();
    match format {
        "json" => Ok(serde_json::to_string_pretty(
            &json!({"mcpServers":{"peripheral":{
                "command":command,"args":["--mcp",endpoint]
            }}}),
        )?),
        "codex" => Ok(format!(
            "[mcp_servers.peripheral]\ncommand = {}\nargs = [\"--mcp\", {}]\nstartup_timeout_sec = 10\ntool_timeout_sec = 150\n",
            serde_json::to_string(&command)?,
            serde_json::to_string(&endpoint)?
        )),
        _ => bail!("unsupported AI client format"),
    }
}

pub fn prompt(workspace: &str, language: &str) -> String {
    if language == "en" {
        format!(
            "Use the configured peripheral MCP server. First call peripheral_docs with language=en, then peripheral_command ping/plugins/instances. Expected workspace: {workspace}. Reuse the running backend; do not create competing device owners. Follow the handbook to implement my requested plugin or debug my device. One logical device per plugin, dynamic Lua settings, truthful current state, cleanup/reconnect and requested UI targets. Validate with mocks before verified hardware queries. Never send unknown Feature/Output reports or invent battery/sleep states. Report tested results and remaining uncertainties. My goal: [describe device, feature and display targets]."
        )
    } else {
        format!(
            "使用已配置的 peripheral MCP。先调用 peripheral_docs（language=zh-CN），再调用 peripheral_command 的 ping/plugins/instances。预期工作区：{workspace}。复用当前后台，不要另起进程抢设备。按手册完成我要求的插件或设备调试：一个逻辑设备一个插件，Lua 动态配置、真实状态、启停清理、身份重连和指定显示目标。先 mock，再使用已确认协议查询；禁止发送未知 Feature/Output 报告，不编造电量或休眠状态。完成后报告验证结果与未验证项。我的需求：[填写设备、功能、显示位置]。"
        )
    }
}
