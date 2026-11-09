use crate::runtime::Runtime;
use serde_json::{Value, json};
fn manifest(id: &str) -> Value {
    json!({"id":id,"name":id,"version":"1","entry":"main.lua","files":["main.lua"],
        "permissions":[{"id":"battery","title":"Battery query","description":"Exact known query only","policy":{"output_rules":[{"vendor_id":1,"product_id":2,"usage_page":3,"usage":4,"interface":0,"packets":["0102"]}]}},
        {"id":"optional","title":"Optional query","description":"Optional","required":false,"policy":{"hid_rules":[{"operation":"get_feature","vendor_id":1,"product_id":2,"usage_page":3,"usage":4,"interface":0,"packets":["0300"]}]}}]})
}
fn install(rt: &Runtime, id: &str) -> Value {
    rt.dispatch(json!({"op":"plugin_install","manifest":manifest(id),"files":{"main.lua":"state.publish('loaded',true)"}})).unwrap()
}
fn grant(rt: &Runtime, id: &str, approved: Value) -> Value {
    let p = rt
        .dispatch(json!({"op":"plugin_permissions","plugin":id}))
        .unwrap();
    rt.dispatch(json!({"op":"plugin_permissions_save","plugin":id,"revision":p["revision"],"approved":approved})).unwrap()
}
#[test]
fn grants_are_isolated_persisted_and_revoke_running_sessions() {
    let root = tempfile::tempdir().unwrap();
    {
        let rt = Runtime::new(root.path()).unwrap();
        assert_eq!(install(&rt, "one")["authorization_required"], true);
        install(&rt, "two");
        assert!(
            rt.dispatch(json!({"op":"instances"}))
                .unwrap()
                .as_array()
                .unwrap()
                .is_empty()
        );
        grant(&rt, "one", json!(["battery"]));
        let p = rt
            .dispatch(json!({"op":"session_policy","instance":"plugin_one"}))
            .unwrap();
        assert_eq!(p["output_rules"][0]["packets"], json!(["0102"]));
        assert_eq!(p["hid_rules"], json!([]));
        assert_eq!(
            rt.dispatch(json!({"op":"plugin_permissions","plugin":"two"}))
                .unwrap()["ready"],
            false
        );
        assert!(
            rt.dispatch(json!({"op":"session_policy","instance":"plugin_two"}))
                .is_err()
        );
        grant(&rt, "one", json!([]));
        assert!(
            rt.dispatch(json!({"op":"session_policy","instance":"plugin_one"}))
                .is_err()
        );
        assert_eq!(
            rt.dispatch(json!({"op":"plugins"})).unwrap()[0]["state"],
            "awaiting_permission"
        );
        grant(&rt, "one", json!(["battery"]));
    }
    let rt = Runtime::new(root.path()).unwrap();
    rt.dispatch(json!({"op":"plugins_restore"})).unwrap();
    assert_eq!(
        rt.dispatch(json!({"op":"plugins"})).unwrap()[0]["running"],
        true
    );
    assert_eq!(
        rt.dispatch(json!({"op":"plugin_permissions","plugin":"one"}))
            .unwrap()["reviewed"],
        true
    );
    let path = root.path().join("plugins/one/plugin.json");
    let mut changed: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    changed["permissions"][0]["policy"]["output_rules"][0]["packets"] = json!(["0103"]);
    let old = rt
        .dispatch(json!({"op":"plugin_permissions","plugin":"one"}))
        .unwrap();
    std::fs::write(path, serde_json::to_vec(&changed).unwrap()).unwrap();
    assert!(rt.dispatch(json!({"op":"plugin_permissions_save","plugin":"one","revision":old["revision"],"approved":["battery"]})).is_err());
    assert_eq!(
        rt.dispatch(json!({"op":"plugin_restart","plugin":"one"}))
            .unwrap()["authorization_required"],
        true
    );
    assert!(
        rt.dispatch(json!({"op":"session_policy","instance":"plugin_one"}))
            .is_err()
    );
}
#[test]
fn explicit_session_policy_overrides_global_and_survives_reset() {
    let root = tempfile::tempdir().unwrap();
    let mut rt = Runtime::new(root.path()).unwrap();
    std::sync::Arc::get_mut(&mut rt.shared).unwrap().policy=serde_json::from_value(json!({"name":"global","output_rules":[{"vendor_id":1,"product_id":2,"usage_page":3,"usage":4,"interface":0,"packets":["0102"]}]})).unwrap();
    rt.dispatch(json!({"op":"create"})).unwrap();
    assert_eq!(
        rt.dispatch(json!({"op":"session_policy"})).unwrap()["name"],
        "global"
    );
    rt.dispatch(json!({"op":"create","instance":"restricted","policy":{}}))
        .unwrap();
    rt.dispatch(json!({"op":"reset","instance":"restricted"}))
        .unwrap();
    assert_eq!(
        rt.dispatch(json!({"op":"session_policy","instance":"restricted"}))
            .unwrap()["output_rules"],
        json!([])
    );
    let mut no_requests = manifest("reader");
    no_requests["permissions"] = json!([]);
    rt.dispatch(json!({"op":"plugin_install","manifest":no_requests,"files":{"main.lua":""}}))
        .unwrap();
    assert_eq!(
        rt.dispatch(json!({"op":"session_policy","instance":"plugin_reader"}))
            .unwrap()["output_rules"],
        json!([])
    );
}
#[test]
fn invalid_or_stale_grants_do_not_stop_a_working_plugin() {
    let root = tempfile::tempdir().unwrap();
    let rt = Runtime::new(root.path()).unwrap();
    install(&rt, "one");
    grant(&rt, "one", json!(["battery"]));
    let p = rt
        .dispatch(json!({"op":"plugin_permissions","plugin":"one"}))
        .unwrap();
    for (revision, approved) in [
        (p["revision"].clone(), json!(["unknown"])),
        (p["revision"].clone(), json!(["battery", "battery"])),
        (json!("stale"), json!([])),
    ] {
        assert!(rt.dispatch(json!({"op":"plugin_permissions_save","plugin":"one","revision":revision,"approved":approved})).is_err());
        assert_eq!(
            rt.dispatch(json!({"op":"lifecycle","instance":"plugin_one"}))
                .unwrap()["running"],
            true
        );
    }
}
