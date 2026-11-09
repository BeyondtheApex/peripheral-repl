//! Mouse bindings share one route across controls, taskbar forwarding and tray icons.
use super::*;
use peripheral_ui_protocol::Events;
pub fn request(p: &Publication, node: Option<&str>, button: &str) -> serde_json::Value {
    json!({"op":"ui_event","instance":p.instance,"generation":p.generation,
        "revision":p.revision,"surface":p.id,"node":node,"button":button})
}
pub(super) fn bind(
    mut element: Stateful<Div>,
    p: &Publication,
    id: Option<&str>,
    events: &Events,
    legacy: &Option<String>,
    commands: &async_channel::Sender<Request>,
) -> Stateful<Div> {
    if !p.surface.interactive {
        return element;
    }
    for (button, name) in [(MouseButton::Left, "left"), (MouseButton::Right, "right")] {
        if events.action(name).is_none() && !(name == "left" && legacy.is_some()) {
            continue;
        }
        let body = json!({"op":"backend_command","command":request(p,id,name),"refresh":false});
        let sender = commands.clone();
        let listener = move |event: &ClickEvent, _: &mut Window, cx: &mut App| {
            if (button == MouseButton::Right) != event.is_right_click() {
                return;
            }
            cx.stop_propagation();
            let (tx, _) = std::sync::mpsc::sync_channel(1);
            let _ = sender.try_send(Request {
                body: body.clone(),
                reply: tx,
                expires: None,
            });
        };
        element = if button == MouseButton::Left {
            element.cursor_pointer().on_click(listener)
        } else {
            element.cursor_pointer().on_aux_click(listener)
        };
    }
    element
}
