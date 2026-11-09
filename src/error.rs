use mlua::{Lua, LuaSerdeExt, Value};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fmt;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fault {
    pub code: String,
    pub message: String,
    pub operation: String,
    pub retryable: bool,
}
impl Fault {
    pub fn new(code: &str, operation: &str, message: impl ToString) -> Self {
        Self {
            code: code.into(),
            operation: operation.into(),
            message: message.to_string().chars().take(4096).collect(),
            retryable: matches!(code, "timeout" | "disconnected" | "busy" | "unavailable"),
        }
    }
    pub fn lua(self) -> mlua::Error {
        mlua::Error::external(self)
    }
}
impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {} ({})", self.code, self.message, self.operation)
    }
}
impl std::error::Error for Fault {}

pub fn detail(error: &mlua::Error) -> Fault {
    match error {
        mlua::Error::ExternalError(e) => e
            .downcast_ref::<Fault>()
            .cloned()
            .unwrap_or_else(|| Fault::new("backend", "lua", e)),
        mlua::Error::CallbackError { cause, .. }
        | mlua::Error::BadArgument { cause, .. }
        | mlua::Error::WithContext { cause, .. } => detail(cause),
        mlua::Error::MemoryError(_) => Fault::new("resource_limit", "lua", error),
        mlua::Error::SyntaxError { .. } => Fault::new("syntax", "lua", error),
        _ => Fault::new("script", "lua", error),
    }
}
pub fn json_error(error: &anyhow::Error) -> serde_json::Value {
    if let Some(f) = error.downcast_ref::<Fault>() {
        return json!(f);
    }
    if let Some(e) = error.downcast_ref::<mlua::Error>() {
        return json!(detail(e));
    }
    json!(Fault::new("runtime", "command", error))
}
pub fn install(lua: &Lua) -> mlua::Result<()> {
    let errors = lua.create_table()?;
    errors.set(
        "info",
        lua.create_function(|lua, value: Value| {
            let fault = match value {
                Value::Error(e) => detail(&e),
                v => Fault::new("script", "lua", v.to_string()?),
            };
            lua.to_value(&fault)
        })?,
    )?;
    errors.set(
        "raise",
        lua.create_function(|_, (code, message): (String, String)| -> mlua::Result<()> {
            Err(Fault::new(&code, "script", message).lua())
        })?,
    )?;
    lua.globals().set("errors", errors)
}
