use super::*;
use crate::{
    bindings::{Context, Policy, Shared},
    runtime::Runtime,
};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

fn policy() -> Policy {
    serde_json::from_value(json!({"gip_write_rules":[{"vendor_id":1,"product_id":2,"message_class":"command","message_id":15,"packets":["f200"]}]})).unwrap()
}
fn eval(rt: &Runtime, code: &str) -> Value {
    rt.dispatch(json!({"op":"eval","code":code,"timeout_ms":5000}))
        .unwrap()
}
fn runtime() -> (tempfile::TempDir, Runtime) {
    let root = tempfile::tempdir().unwrap();
    let rt = Runtime::new(root.path()).unwrap();
    rt.dispatch(json!({"op":"create","policy":policy()}))
        .unwrap();
    (root, rt)
}

#[test]
fn replay_uses_shared_native_budget_and_releases_it_on_close() {
    let (_root, rt) = runtime();
    eval(
        &rt,
        r#"
        local channels={}
        local steps={{op='battery_info',result={padding=string.rep('x',220000)}}}
        for i=1,4 do channels[i]=mock.gip(steps) end
        local ok,e=pcall(function()mock.gip(steps)end)
        assert(not ok and errors.info(e).code=='resource_limit')
        for _,channel in ipairs(channels)do channel:close()end
        local last=mock.gip(steps);last:close()
    "#,
    );
}

#[test]
fn delayed_queue_delivery_preserves_monotonic_age() {
    let (_root, rt) = runtime();
    eval(
        &rt,
        r#"
        local d=mock.gip({{op='send',message_class='command',message_id=15,payload_hex='f200',messages={{message_class='command',message_id=16,payload_hex='e050'}}}},{vendor_id=1,product_id=2})
        d:send('command',15,buffer.hex('f200'));runtime.sleep(80)
        local m=d:read_message(0);assert(m.age_ms>=60);d:close()
    "#,
    );
}

#[test]
fn policy_is_exact_and_old_policy_serialization_stays_compatible() {
    let rule = &policy().gip_write_rules[0];
    assert!(rule.allows(1, 2, MessageClass::Command, 15, &[0xf2, 0]));
    for (vendor, product, class, id, payload) in [
        (3, 2, MessageClass::Command, 15, vec![0xf2, 0]),
        (1, 3, MessageClass::Command, 15, vec![0xf2, 0]),
        (1, 2, MessageClass::LowLatency, 15, vec![0xf2, 0]),
        (1, 2, MessageClass::Command, 16, vec![0xf2, 0]),
        (1, 2, MessageClass::Command, 15, vec![0xf2, 1]),
    ] {
        assert!(!rule.allows(vendor, product, class, id, &payload));
    }
    assert!(
        serde_json::to_value(Policy::default())
            .unwrap()
            .get("gip_write_rules")
            .is_none()
    );
    let mut rules = policy().gip_write_rules;
    rules[0].packets = vec!["F200".into()];
    assert!(validate_rules(&rules).is_err());
    rules[0].packets = vec!["00".repeat(4097)];
    assert!(validate_rules(&rules).is_err());
}

#[test]
fn lua_request_subscribes_before_send_and_does_not_consume_other_subscribers() {
    let (_root, rt) = runtime();
    let result = eval(
        &rt,
        r#"
        d=mock.gip({{op='send',message_class='command',message_id=15,payload_hex='f200',messages={
            {message_class='low_latency',message_id=0,payload_hex='0000'},
            {message_class='command',message_id=16,payload_hex='e04e',sequence=9,timestamp=42}
        }}},{vendor_id=1,product_id=2})
        local s=d:subscribe{message_class='command',message_id=16}
        local r=d:request('command',15,buffer.hex('f200'),function(m)
            return m.kind=='message' and m.message_id==16
        end,2000)
        assert(r.payload:hex()=='e04e' and r.actual_length==2 and r.sequence==9 and r.timestamp==42)
        assert(s:next(0).payload:hex()=='e04e')
        assert(s:next(0).kind=='timeout')
        assert(d:read_message(0).message_class=='low_latency')
        d:close()
        local ok,e=pcall(function() return s:next(0) end)
        assert(not ok and errors.info(e).code=='closed')
        return true
    "#,
    );
    assert_eq!(result["values"], json!([true]));
}

#[test]
fn denied_writes_do_not_consume_replay_steps() {
    let (_root, rt) = runtime();
    eval(
        &rt,
        r#"
        d=mock.gip({{op='send',message_class='command',message_id=15,payload_hex='f200'}},{vendor_id=1,product_id=2})
        for _,p in ipairs{{'command',15,'f201'},{'command',16,'f200'},{'low_latency',15,'f200'}} do
            local ok,e=pcall(function() d:send(p[1],p[2],buffer.hex(p[3])) end)
            assert(not ok and errors.info(e).code=='policy_denied')
        end
        local ok,e=pcall(function() d:send('command',15,buffer.hex('f200'),0) end)
        assert(not ok and errors.info(e).code=='invalid_argument')
        assert(d:send('command',15,buffer.hex('f200')).sent)
    "#,
    );
}

#[test]
fn raw_system_battery_never_gets_promoted_to_exact_percentage() {
    let (_root, rt) = runtime();
    eval(
        &rt,
        r#"
        local d=mock.gip{{op='battery_info',result={source='windows_gaming_input',precision='driver_defined',remaining_capacity_mwh=700,full_charge_capacity_mwh=1000}}}
        local r=d:battery_info()
        assert(r.remaining_capacity_mwh==700 and r.full_charge_capacity_mwh==1000 and r.percent==nil)
        d:close()
    "#,
    );
}

#[test]
fn disconnect_does_not_return_queued_telemetry_as_current() {
    let (_root, rt) = runtime();
    eval(
        &rt,
        r#"
        local d=mock.gip({{op='send',message_class='command',message_id=15,payload_hex='f200',disconnect=true,messages={{message_class='command',message_id=16,payload_hex='e050'}}}},{vendor_id=1,product_id=2})
        local s=d:subscribe()
        d:send('command',15,buffer.hex('f200'))
        for _,fn in ipairs{function() return d:read_message(0) end,function() return s:next(0) end} do
            local ok,e=pcall(fn);assert(not ok and errors.info(e).code=='disconnected')
        end
        d:close()
    "#,
    );
}

#[test]
fn stop_closes_channels_and_reset_releases_device_leases() {
    let (_root, rt) = runtime();
    eval(&rt, "d=mock.gip({}); s=d:subscribe(); return d:inspect()");
    assert!(!rt.shared.leases.lock().unwrap().is_empty());
    rt.dispatch(json!({"op":"stop"})).unwrap();
    assert!(rt.shared.leases.lock().unwrap().is_empty());
    rt.dispatch(json!({"op":"reset"})).unwrap();
    assert_eq!(eval(&rt, "return d==nil")["values"], json!([true]));
}

#[test]
fn native_timeout_retains_exclusive_lease_until_worker_completes() {
    crate::bluetooth::executor().block_on(async{
        let shared=Arc::new(Shared::default());let mut ctx=Context::new("a".into(),shared.clone(),std::env::temp_dir());ctx.policy=Arc::new(policy());
        let info=json!({"path":"gip:test-timeout","vendor_id":1,"product_id":2});
        let steps=VecDeque::from([json!({"op":"send","message_class":"command","message_id":15,"payload_hex":"f200","delay_ms":250})]);
        let d=channel::Core::new(&ctx,info.clone(),channel::Backend::Mock(Mutex::new(steps)),Arc::new(feed::Feed::default()),feed::MessageFilter::default()).unwrap();
        assert_eq!(d.send(MessageClass::Command,15,vec![0xf2,0],30).await.unwrap_err().code,"timeout");
        d.close();assert!(shared.leases.lock().unwrap().contains_key("gip:test-timeout"));
        assert!(channel::Core::new(&ctx,info,channel::Backend::Mock(Mutex::new(VecDeque::new())),Arc::new(feed::Feed::default()),feed::MessageFilter::default()).is_err());
        tokio::time::sleep(Duration::from_millis(350)).await;
        assert!(!shared.leases.lock().unwrap().contains_key("gip:test-timeout"));
    });
}

#[test]
fn queues_are_bounded_and_filter_only_messages() {
    crate::bluetooth::executor().block_on(async {
        let f = feed::Feed::default();
        let q = f
            .subscribe(feed::MessageFilter {
                message_class: Some(MessageClass::Command),
                message_id: Some(16),
            })
            .unwrap();
        f.message(0, 1, 0, 0, &[0]);
        assert_eq!(q.next(0).await.unwrap()["kind"], "timeout");
        f.emit(json!({"kind":"input_suspended"}));
        assert_eq!(q.next(0).await.unwrap()["kind"], "input_suspended");
        for _ in 0..150 {
            f.message(0, 0, 16, 0, &[0xe0]);
        }
        assert_eq!(q.info()["queued"], 128);
        assert_eq!(
            q.next(0).await.unwrap(),
            json!({"kind":"overflow","dropped":22})
        );
        f.disconnect();
        assert!(q.is_closed());
        assert!(f.subscribe(Default::default()).is_err());
    });
}

#[test]
fn replay_rejects_id_wrapping_and_oversized_frames() {
    for step in [
        json!({"op":"send","message_class":"command","message_id":256,"payload_hex":"f200"}),
        json!({"op":"battery_info","messages":[{"message_class":"command","message_id":16,"sequence":300,"payload_hex":"00"}]}),
        json!({"op":"battery_info","delay_ms":5001}),
        json!({"op":"battery_info","messages":[{"message_class":"command","message_id":16,"payload_hex":"00".repeat(4097)}]}),
    ] {
        assert!(replay::validate(&[step]).is_err());
    }
    let (_root, rt) = runtime();
    eval(
        &rt,
        "assert(not pcall(function() mock.gip({}, {vendor_id=65536}) end))",
    );
}

#[test]
fn plugin_grants_are_required_and_remain_session_scoped() {
    let root = tempfile::tempdir().unwrap();
    let rt = Runtime::new(root.path()).unwrap();
    let permissions = json!([{"id":"known-query","title":"Known command","description":"Exact replay packet only","required":true,"policy":policy()}]);
    rt.dispatch(json!({"op":"plugin_install","manifest":{"id":"gip-test","name":"GIP test","version":"1","entry":"main.lua","files":["main.lua"],"permissions":permissions},"files":{"main.lua":"d=mock.gip({{op='send',message_class='command',message_id=15,payload_hex='f200'}},{vendor_id=1,product_id=2}); d:send('command',15,buffer.hex('f200')); d:close()"}})).unwrap();
    let review = rt
        .dispatch(json!({"op":"plugin_permissions","plugin":"gip-test"}))
        .unwrap();
    assert_eq!(review["ready"], false);
    let saved=rt.dispatch(json!({"op":"plugin_permissions_save","plugin":"gip-test","revision":review["revision"],"approved":["known-query"]})).unwrap();
    assert!(saved["startup_error"].is_null(), "{saved}");
    rt.dispatch(json!({"op":"create"})).unwrap();
    assert!(
        rt.dispatch(json!({"op":"session_policy"}))
            .unwrap()
            .get("gip_write_rules")
            .is_none()
    );
}
