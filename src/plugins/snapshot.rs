use super::*;
pub(super) fn registration_only(device: Value) -> Value {
    // Whitelist registration/artwork only, never derived display, samples or
    // freshness/generation metadata from a previous runtime.
    json!({"descriptor":device["descriptor"],"image":device["image"],
        "resource":device["resource"],"state":{},"stale":true})
}
pub(super) fn plugin(
    runtime: &Runtime,
    manifest: &package::Manifest,
    state: &State,
    root: &Path,
) -> Value {
    let instance = session(&manifest.id);
    let permissions = super::permissions::describe(root, manifest).unwrap_or_else(
        |e| json!({"ready":false,"reviewed":false,"error":e.to_string(),"items":[]}),
    );
    let setting = state
        .settings
        .get(&manifest.id)
        .cloned()
        .unwrap_or_default();
    let stats = runtime
        .dispatch(json!({"op":"stats","instance":instance}))
        .ok();
    let running = runtime
        .dispatch(json!({"op":"lifecycle","instance":instance}))
        .ok()
        .is_some_and(|v| v["running"] == true);
    let devices = if stats.is_some() {
        runtime
            .dispatch(json!({"op":"snapshot","instance":instance}))
            .unwrap_or(json!({}))
    } else {
        json!({})
    };
    let device = runtime
        .dispatch(json!({"op":"device_snapshot","instance":instance}))
        .ok()
        .filter(|v| !v.is_null())
        .or(setting.device.clone());
    let views = runtime.shared.ui.lock().unwrap().surfaces.values().filter(|p| p.instance == instance).map(|p| json!({"id":p.id,"title":p.surface.title,"target":p.surface.target,"publication":p})).collect::<Vec<_>>();
    let description = runtime
        .dispatch(json!({"op":"config_description","instance":instance}))
        .unwrap_or(
            json!({"value":setting.config.clone().unwrap_or(json!({})),"schema":setting.schema}),
        );
    json!({"id":manifest.id,"name":manifest.name,"version":manifest.version,"description":manifest.description,
        "permissions":permissions,"enabled":setting.enabled,"running":running,"state":if setting.enabled && permissions["ready"] != true {"awaiting_permission"}else if state.errors.contains_key(&manifest.id){"error"}else if running{"running"}else{"stopped"},
        "error":state.errors.get(&manifest.id),"instance":instance,"config":description["value"],"config_schema":description["schema"],
        "config_live":stats.is_some(),"devices":devices,"device":device,"views":views,"stats":stats})
}
