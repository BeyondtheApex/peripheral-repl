use crate::runtime::Runtime;
use serde_json::{Value, json};

fn publication(rt: &Runtime) -> Value {
    rt.dispatch(json!({"op":"ui_snapshot"})).unwrap()["surfaces"][0].clone()
}

#[test]
fn lua_position_defaults_validation_updates_and_event_routing() {
    let root = tempfile::tempdir().unwrap();
    let rt = Runtime::new(root.path()).unwrap();
    rt.dispatch(json!({"op":"create","instance":"order_test"}))
        .unwrap();
    let eval = |code: &str| rt.dispatch(json!({"op":"eval","instance":"order_test","code":code}));
    eval(
        r#"
        ui.on('left',{type='object'},function(e) state.publish('clicked',e.button) end)
        ui.on('right',{type='object'},function(e) state.publish('clicked',e.button) end)
        ui.publish('view',{target='taskbar',events={left_click='left',right_click='right'},
            root={id='label',type='text',text='unchanged'}})
        local pos=ui.list()[1].surface.position_preference
        assert(pos[1]==0 and pos[2]==0)
    "#,
    )
    .unwrap();
    let before = publication(&rt);
    eval("ui.position('view',-10,25)").unwrap();
    let after = publication(&rt);
    assert_eq!(after["surface"]["position_preference"], json!([-10, 25]));
    assert_eq!(before["generation"], after["generation"]);
    assert_eq!(before["surface"]["root"], after["surface"]["root"]);
    assert_ne!(before["revision"], after["revision"]);
    eval("ui.position('view',-10,25)").unwrap();
    assert_eq!(after, publication(&rt), "unchanged preferences are a no-op");
    for code in [
        "ui.position('view',1.5,0)",
        "ui.position('view',2147483648,0)",
        "ui.position('view',0/0,0)",
        "ui.position('missing',0,0)",
        "ui.publish('bad',{target='taskbar',position_preference={1},root={id='r',type='text'}})",
    ] {
        assert!(eval(code).is_err(), "accepted invalid preference: {code}");
        assert_eq!(after, publication(&rt));
    }
    for button in ["left", "right"] {
        rt.dispatch(
            json!({"op":"ui_event","instance":"order_test","surface":"view",
            "generation":after["generation"],"revision":after["revision"],"button":button}),
        )
        .unwrap();
        assert_eq!(
            rt.dispatch(json!({"op":"snapshot","instance":"order_test"}))
                .unwrap()["clicked"],
            button
        );
    }
    assert!(
        rt.dispatch(
            json!({"op":"ui_event","instance":"order_test","surface":"view",
        "generation":before["generation"],"revision":before["revision"],"button":"left"})
        )
        .is_err()
    );
    eval("ui.position('view',0,0)").unwrap();
    assert_eq!(
        publication(&rt)["surface"]["position_preference"],
        json!([0, 0])
    );
}
