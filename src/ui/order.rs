//! Renderer ordering independent of publish timing, revisions and session generations.
use super::Publication;
use std::cmp::Ordering;

fn identity(a: &Publication, b: &Publication) -> Ordering {
    (&a.instance, &a.id).cmp(&(&b.instance, &b.id))
}

pub fn taskbar(items: &mut [Publication]) {
    items.sort_by(|a, b| {
        a.surface.position_preference[0]
            .cmp(&b.surface.position_preference[0])
            .then_with(|| identity(a, b))
    });
}

pub fn overview(items: &mut [Publication]) {
    items.sort_by(|a, b| {
        // Compare in reverse instead of negating: i32::MIN is a valid preference.
        b.surface.position_preference[1]
            .cmp(&a.surface.position_preference[1])
            .then_with(|| identity(a, b))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn item(instance: &str, id: &str, position: Option<[i32; 2]>) -> Publication {
        let mut surface = json!({"target":"taskbar","root":{"id":"label","type":"text"}});
        if let Some(position) = position {
            surface["position_preference"] = json!(position);
        }
        Publication {
            instance: instance.into(),
            id: id.into(),
            revision: 1,
            generation: "first".into(),
            resources: Default::default(),
            surface: serde_json::from_value(surface).unwrap(),
        }
    }
    fn ids(items: &[Publication]) -> Vec<String> {
        items
            .iter()
            .map(|p| format!("{}/{}", p.instance, p.id))
            .collect()
    }
    #[test]
    fn signed_preferences_and_stable_ties_ignore_startup_order() {
        let original = vec![
            item("b", "same", None),
            item("a", "z", Some([0, 0])),
            item("a", "a", None),
            item("low", "v", Some([i32::MIN, i32::MIN])),
            item("high", "v", Some([i32::MAX, i32::MAX])),
        ];
        for shift in 0..original.len() {
            let mut items = original.clone();
            items.rotate_left(shift);
            for p in &mut items {
                p.generation = "restarted".into();
                p.revision += 50;
            }
            taskbar(&mut items);
            assert_eq!(ids(&items), ["low/v", "a/a", "a/z", "b/same", "high/v"]);
            for target in [
                peripheral_ui_protocol::Target::TrayCard,
                peripheral_ui_protocol::Target::TrayList,
            ] {
                for p in &mut items {
                    p.surface.target = target;
                }
                overview(&mut items);
                assert_eq!(ids(&items), ["high/v", "a/a", "a/z", "b/same", "low/v"]);
            }
        }
    }
    #[test]
    fn preferences_are_independent_and_updates_reorder_without_changing_identity() {
        let mut items = vec![
            item("a", "v", Some([10, 20])),
            item("b", "v", Some([-5, -10])),
        ];
        taskbar(&mut items);
        assert_eq!(ids(&items), ["b/v", "a/v"]);
        overview(&mut items);
        assert_eq!(ids(&items), ["a/v", "b/v"]);
        items[1].surface.position_preference = [-5, 30];
        overview(&mut items);
        assert_eq!(ids(&items), ["b/v", "a/v"]);
        assert!(
            items
                .iter()
                .all(|p| p.revision == 1 && p.generation == "first")
        );
    }
}
