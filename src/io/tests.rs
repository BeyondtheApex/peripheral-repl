//! Hardware-free transport contracts; captured frames never authorize device writes.
use crate::runtime::Runtime;
use serde_json::json;

fn evaluate(code: &str) {
    let root = tempfile::tempdir().unwrap();
    let rt = Runtime::new(root.path()).unwrap();
    rt.dispatch(json!({"op":"create"})).unwrap();
    rt.dispatch(json!({"op":"eval","code":code})).unwrap();
}

#[test]
fn unnumbered_control_and_interrupt_packets_preserve_payload_bytes() {
    evaluate(
        r#"
        local c=mock.replay({
            {op='get_feature',request='000000',response='00a131'},
            {op='get_input_report',request='000000',response='003234'},
            {request='0083',response='a130'},
        },{numbered_reports=false})
        local f=c:get_feature(0,3)
        assert(f.control_wire:hex()=='00a131' and f.wire:hex()=='a131')
        assert(f.payload:hex()=='a131' and f.actual_length==2)
        assert(f.report_id==0 and f.requested_report_id==0 and f.report_id_matches)
        local i=c:get_input_report(0,3)
        assert(i.wire:hex()=='3234' and i.payload:hex()=='3234')
        assert(i.control_wire:hex()=='003234')
        c:write_output(0,buffer.hex('83'))
        local p=c:read_input(2,500)
        assert(p.status=='ok' and p.wire:hex()=='a130' and p.payload:hex()=='a130')
        assert(p.control_wire==nil and p.report_id==0)
        c:close()
    "#,
    );
}

#[test]
fn unnumbered_reports_reject_nonzero_ids_without_consuming_replay_steps() {
    evaluate(
        r#"
        local c=mock.replay({{request='0083',response='a130'}},{numbered_reports=false})
        for _,fn in ipairs{
            function() c:write_output(1,buffer.hex('83')) end,
            function() c:request(1,buffer.hex('83'),function() return true end) end,
            function() c:get_input_report(1,3) end,
            function() c:get_feature(1,3) end,
            function() c:set_feature(1,buffer.hex('83')) end,
            function() c:send_output_report(1,buffer.hex('83')) end,
        } do
            local ok,e=pcall(fn)
            assert(not ok and errors.info(e).code=='invalid_argument')
        end
        local p=c:request(0,buffer.hex('83'),function(p) return p.payload:hex()=='a130' end)
        assert(p.wire:hex()=='a130')
        c:close()
    "#,
    );
}

#[test]
fn feature_only_replay_supports_controls_and_rejects_input_apis() {
    evaluate(
        r#"
        local c=mock.replay({
            {op='set_feature',request='0083'},
            {op='get_feature',request='000000',response='00a131'},
        },{numbered_reports=false,input_reports=false})
        local cap=c:inspect().capabilities
        assert(not cap.read_input and not cap.get_input_report and not cap.subscriptions)
        for _,fn in ipairs{
            function() c:subscribe() end,
            function() c:read_input(8,1) end,
            function() c:get_input_report(0,3) end,
            function() c:request(0,buffer.hex('83'),function() return true end) end,
        } do
            local ok,e=pcall(fn)
            assert(not ok and errors.info(e).code=='unsupported')
        end
        c:transaction(function(tx)
            assert(tx:set_feature(0,buffer.hex('83'))==2)
            assert(tx:get_feature(0,3).payload:hex()=='a131')
        end)
        c:close()
    "#,
    );
}

#[test]
fn reconnect_preserves_framing_with_explicit_and_resolved_references() {
    evaluate(
        r#"
        local original_open,original_resolve=device.open,device.resolve
        local closed,resolved=false,false
        local reference={path='selected'}
        local channel={
            inspect=function() return {device={path='old'},capabilities={numbered_reports=false}} end,
            close=function() closed=true end,
        }
        device.resolve=function(old)
            assert(closed and old.path=='old');resolved=true;return reference
        end
        device.open=function(ref,opts)
            assert(closed and ref==reference and opts.numbered_reports==false)
            return 'new-channel'
        end
        assert(device.reconnect(channel,reference)=='new-channel' and not resolved)
        closed=false
        assert(device.reconnect(channel)=='new-channel' and resolved)
        device.open,device.resolve=original_open,original_resolve
    "#,
    );
}

#[test]
fn control_vendor_header_is_explicit_and_feature_ids_remain_strict() {
    evaluate(
        r#"
        local c=mock.replay{
            {op='get_input_report',request='ba0000',response='010142'},
            {op='get_feature',request='ba0000',response='010142'},
            {op='get_feature',request='ba0000',response='ba42'},
        }
        local p=c:get_input_report(0xba,3)
        assert(p.requested_report_id==0xba and not p.report_id_matches)
        assert(p.control_wire:hex()=='010142' and p.wire:hex()=='010142')
        local ok,e=pcall(function() c:get_feature(0xba,3) end)
        assert(not ok and errors.info(e).code=='invalid_frame')
        local f=c:get_feature(0xba,3)
        assert(f.actual_length==2 and f.report_id_matches and f.payload:hex()=='42')
        c:close()
    "#,
    );
}

#[test]
fn interrupt_timeout_and_overlong_reports_never_fabricate_a_packet() {
    evaluate(
        r#"
        local c=mock.replay{{request='0701',response='07313233'}}
        local p=c:read_input(2,1)
        assert(p.status=='timeout' and p.actual_length==0)
        c:write_output(7,buffer.hex('01'))
        local ok,e=pcall(function() c:read_input(2,500) end)
        assert(not ok and errors.info(e).code=='short_buffer')
        c:close()
    "#,
    );
}

#[test]
fn delayed_control_timeout_does_not_poison_the_next_response() {
    evaluate(
        r#"
        local c=mock.replay{
            {op='get_feature',request='0700',response='0731',delay_ms=200},
            {op='get_feature',request='0700',response='0732'},
        }
        local ok,e=pcall(function()
            c:transaction(function(tx) tx:get_feature(7,2) end,40)
        end)
        assert(not ok and errors.info(e).code=='timeout')
        assert(c:get_feature(7,2).payload:hex()=='32')
        c:close()
    "#,
    );
}

#[test]
fn disconnect_and_close_invalidate_future_controls_and_subscriptions() {
    evaluate(
        r#"
        local c=mock.replay{{op='get_feature',request='0700',disconnect=true}}
        local observer=c:subscribe()
        local ok,e=pcall(function() c:get_feature(7,2) end)
        assert(not ok and errors.info(e).code=='disconnected')
        assert(c:inspect().closed)
        for _,fn in ipairs{
            function() c:get_feature(7,2) end,
            function() c:write_output(7,buffer.hex('01')) end,
            function() c:subscribe() end,
        } do
            local success,err=pcall(fn)
            assert(not success and errors.info(err).code=='closed')
        end
        c:close();c:close();observer:close()
    "#,
    );
}

#[test]
fn queued_commands_are_discarded_after_a_disconnect() {
    use super::*;
    let (tx, rx) = mpsc::sync_channel(16);
    let core = Arc::new(ChannelCore {
        tx: tx.clone(),
        closed: AtomicBool::new(false),
        finished: AtomicBool::new(false),
        subscribers: Mutex::new(vec![]),
        input: Queue::new(8),
        gate: tokio::sync::Mutex::new(()),
        info: json!({"backend":"mock"}),
        path: "mock".into(),
        owner: "test".into(),
        shared: Arc::new(Shared::default()),
        policy: Arc::new(bindings::Policy::default()),
        generation: "test".into(),
        numbered: true,
        input_reports: true,
    });
    let step = |op: &str, request: Vec<u8>, disconnect| Step {
        op: op.into(),
        request,
        disconnect,
        error: None,
        response: None,
        delay: Duration::ZERO,
    };
    let backend = Backend::Mock {
        steps: VecDeque::from([
            step("get_feature", vec![7, 0], true),
            step("write_output", vec![7, 1], false),
        ]),
        pending: vec![],
        input_reports: true,
        _allocation: bindings::Charge::new(Arc::new(Default::default()), 0).unwrap(),
    };
    let (first_tx, mut first_rx) = tokio::sync::oneshot::channel();
    let (second_tx, mut second_rx) = tokio::sync::oneshot::channel();
    tx.send(Command::Control {
        op: "get_feature".into(),
        wire: vec![7, 0],
        reply: first_tx,
        cancel: Default::default(),
    })
    .unwrap();
    tx.send(Command::Write(vec![7, 1], second_tx, Default::default()))
        .unwrap();
    worker(core.clone(), backend, rx);
    assert_eq!(
        first_rx.try_recv().unwrap().unwrap_err().code,
        "disconnected"
    );
    assert_eq!(
        second_rx.try_recv().unwrap_err(),
        tokio::sync::oneshot::error::TryRecvError::Closed
    );
    assert!(core.finished.load(Ordering::Acquire));
    let events = core.shared.events.lock().unwrap().read(0);
    assert!(
        !events["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["data"]["op"] == "write_output")
    );
}
