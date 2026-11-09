//! Offline regression cases for plugin ownership, stale data and presentation.
use crate::runtime::Runtime;
use serde_json::{Value, json};

fn install(runtime: &Runtime, id: &str, source: &str) {
    let result = runtime.dispatch(json!({"op":"plugin_install",
        "manifest":{"id":id,"name":id,"version":"1","entry":"main.lua","files":["main.lua"],
            "default_locale":"zh-CN","locales":{"zh-CN":{"online":"在线"},"en":{"online":"Online"}}},
        "files":{"main.lua":source}})).unwrap();
    assert!(result["startup_error"].is_null(), "{result}");
}
fn device(runtime: &Runtime) -> Value {
    runtime
        .dispatch(json!({"op":"device_snapshot","instance":"plugin_sample"}))
        .unwrap()
}
const SAMPLE: &str = "device.register{id='one',name='Device'}; device.update{status='online',label='online',connected=true,battery_percent=73,values={battery=73},ttl_ms=100}";

#[test]
fn language_refresh_preserves_sample_and_expiry() {
    let root = tempfile::tempdir().unwrap();
    let rt = Runtime::new(root.path()).unwrap();
    install(&rt, "sample", SAMPLE);
    let before = device(&rt);
    std::thread::sleep(std::time::Duration::from_millis(110));
    rt.dispatch(
        json!({"op":"ui_environment","value":{"locale":"en","theme":"light","accent":"#2984e3"}}),
    )
    .unwrap();
    let after = device(&rt);
    assert_eq!(after["display"]["label"], "Online");
    for field in [
        "state",
        "updated_unix_ms",
        "last_seen_unix_ms",
        "generation",
    ] {
        assert_eq!(before[field], after[field]);
    }
    assert_eq!(after["stale"], true);
}

#[test]
fn persisted_registration_excludes_telemetry_and_migrates_old_preferences() {
    let root = tempfile::tempdir().unwrap();
    {
        let rt = Runtime::new(root.path()).unwrap();
        install(&rt, "sample", SAMPLE);
        let path = root.path().join(".peripheral/plugins.json");
        let mut saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let d = &saved["sample"]["device"];
        assert_eq!(d["descriptor"]["id"], "one");
        for field in [
            "display",
            "generation",
            "updated_unix_ms",
            "last_seen_unix_ms",
        ] {
            assert!(d.get(field).is_none());
        }
        assert_eq!(d["state"], json!({}));
        saved["sample"]["device"]["display"] = json!({"values":[{"value":73}]});
        saved["sample"]["device"]["state"] = json!({"battery_percent":73});
        std::fs::write(path, serde_json::to_vec(&saved).unwrap()).unwrap();
    }
    let rt = Runtime::new(root.path()).unwrap();
    let plugins = rt.dispatch(json!({"op":"plugins"})).unwrap();
    assert!(plugins[0]["device"]["display"].is_null());
    assert_eq!(plugins[0]["device"]["state"], json!({}));
    assert_eq!(plugins[0]["device"]["stale"], true);
}

#[test]
fn repl_reset_cannot_destroy_or_reparent_a_managed_plugin() {
    let root = tempfile::tempdir().unwrap();
    let rt = Runtime::new(root.path()).unwrap();
    install(&rt, "sample", SAMPLE);
    let before = device(&rt);
    for op in ["create", "reset", "reload", "load"] {
        assert!(
            rt.dispatch(json!({"op":op,"instance":"plugin_sample","path":"main.lua"}))
                .is_err()
        );
        assert_eq!(device(&rt)["generation"], before["generation"]);
    }
    rt.dispatch(json!({"op":"plugin_restart","plugin":"sample"}))
        .unwrap();
    assert_ne!(device(&rt)["generation"], before["generation"]);
}

#[test]
fn failing_environment_callback_does_not_block_other_plugins() {
    let root = tempfile::tempdir().unwrap();
    let rt = Runtime::new(root.path()).unwrap();
    install(
        &rt,
        "bad",
        "ui.on_environment(function() error('failure') end)",
    );
    install(
        &rt,
        "good",
        "ui.on_environment(function(e) state.publish('locale',e.locale) end)",
    );
    let response=rt.dispatch(json!({"op":"ui_environment","value":{"locale":"en","theme":"dark","accent":"#2984e3"}})).unwrap();
    assert!(response["callback_errors"]["plugin_bad"].is_string());
    assert_eq!(
        rt.dispatch(json!({"op":"snapshot","instance":"plugin_good"}))
            .unwrap()["locale"],
        "en"
    );
}
