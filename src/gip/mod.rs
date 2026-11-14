//! Generic GIP transport. Device protocols, heartbeats and telemetry belong in Lua.
mod background;
mod channel;
mod diagnostics;
mod feed;
mod lua;
mod policy;
mod replay;
#[cfg(test)]
mod tests;
pub use lua::install;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use self::windows as native;
#[cfg(not(windows))]
mod unsupported;
pub use policy::{MessageClass, WriteRule, validate_rules};
#[cfg(not(windows))]
use unsupported as native;

pub fn capabilities() -> serde_json::Value {
    serde_json::json!({"available":cfg!(windows),"raw_messages":true,"automatic_heartbeats":false,
        "host":crate::host::status(),"background_api":"gip.background() requires installed GameInput v4; delivery may additionally require elevation",
        "firmware_update":false,"background_delivery":"Windows driver and process privilege dependent",
        "aggregation_abi":"Windows custom-controller aggregation; OS compatibility must be verified"})
}
pub async fn list(filter: feed::Filter) -> Result<Vec<serde_json::Value>, crate::error::Fault> {
    filter.validate()?;
    let f = filter.clone();
    tokio::task::spawn_blocking(move || native::discover(&f))
        .await
        .map_err(|e| crate::error::Fault::new("backend", "gip.list", e))??;
    tokio::time::sleep(std::time::Duration::from_millis(
        filter.wait_ms.unwrap_or(500),
    ))
    .await;
    snapshot(filter).await
}
async fn snapshot(filter: feed::Filter) -> Result<Vec<serde_json::Value>, crate::error::Fault> {
    tokio::task::spawn_blocking(move || native::snapshot(&filter))
        .await
        .map_err(|e| crate::error::Fault::new("backend", "gip.list", e))?
}
pub fn command_sync(command: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    match command["action"].as_str().unwrap_or("list") {
        "capabilities" => Ok(capabilities()),
        "diagnostics" => Ok(diagnostics::snapshot()),
        "list" => {
            let mut value = command
                .get("filter")
                .cloned()
                .unwrap_or(serde_json::json!({}));
            if let Some(object) = value.as_object_mut() {
                object.remove("backend");
            }
            let filter = serde_json::from_value(value)?;
            Ok(serde_json::json!(
                crate::bluetooth::executor().block_on(list(filter))?
            ))
        }
        _ => anyhow::bail!("unknown GIP action"),
    }
}
