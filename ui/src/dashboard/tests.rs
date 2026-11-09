//! Rendered layout regression: alignment applies to the grid, never each row.
use super::{Alignment, Dashboard, Mode, Options};
use crate::control::Request;
use gpui_core::{
    self as gpui, Context, IntoElement, Render, TestAppContext, VisualTestContext, Window, px, size,
};
use peripheral_repl::ui::Publication;
use serde_json::json;

#[gpui::test]
fn incomplete_rows_keep_their_columns_when_the_grid_is_aligned(cx: &mut TestAppContext) {
    struct Fixture {
        dashboard: Dashboard,
        items: Vec<Publication>,
        sender: async_channel::Sender<Request>,
    }
    impl Render for Fixture {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.dashboard
                .render(&self.items, &self.sender, window.viewport_size())
        }
    }
    cx.update(gpui_kit::init);
    let (sender, _) = async_channel::bounded(8);
    let window = cx.add_window(|_, _| Fixture {
        dashboard: Dashboard::default(),
        items: Vec::new(),
        sender,
    });
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    // Exercise both natural and proportionally reduced cards, including odd
    // widths that used to risk flex wrapping due to fractional pixel sizes.
    for width in [960., 441., 320.] {
        visual.simulate_resize(size(px(width), px(600.)));
        for alignment in [Alignment::Start, Alignment::Center, Alignment::End] {
            for columns in [2, 3, 4] {
                window
                    .update(&mut visual, |fixture, _, cx| {
                        fixture.dashboard.options = Options {
                            mode: Mode::Grid,
                            columns,
                            rows: 3,
                            horizontal_alignment: alignment,
                            ..Options::default()
                        };
                        fixture.items = (0..columns + 1)
                            .map(|id| Publication {
                                instance: "test".into(),
                                id: id.to_string(),
                                revision: 1,
                                generation: "test".into(),
                                resources: Default::default(),
                                surface: serde_json::from_value(json!({"target":"tray_card",
                            "title":"Test", "root":{"id":"label","type":"text","text":"Test"}}))
                                .unwrap(),
                            })
                            .collect();
                        cx.notify();
                    })
                    .unwrap();
                visual.update(|window, cx| {
                    window.draw(cx).clear(cx);
                });
                let first = visual.debug_bounds("dashboard-item-0").unwrap();
                let second = visual.debug_bounds("dashboard-item-1").unwrap();
                let row_end = visual
                    .debug_bounds(match columns {
                        2 => "dashboard-item-1",
                        3 => "dashboard-item-2",
                        _ => "dashboard-item-3",
                    })
                    .unwrap();
                let last = visual
                    .debug_bounds(match columns {
                        2 => "dashboard-item-2",
                        3 => "dashboard-item-3",
                        _ => "dashboard-item-4",
                    })
                    .unwrap();
                assert!(
                    (f32::from(first.left()) - f32::from(last.left())).abs() < 0.1,
                    "incomplete row moved: {alignment:?}, {columns} columns, width {width}"
                );
                assert!((f32::from(first.top()) - f32::from(second.top())).abs() < 0.1);
                assert!(last.top() > first.top());
                let grid_width = f32::from(row_end.right() - first.left());
                let expected_left = match alignment {
                    Alignment::Start => 16.,
                    Alignment::Center => (width - grid_width) / 2.,
                    Alignment::End => width - 16. - grid_width,
                };
                assert!(
                    (f32::from(first.left()) - expected_left).abs() < 1.,
                    "whole-grid alignment: {alignment:?}, {columns} columns, width {width}, grid width {grid_width}, expected {expected_left}, first {first:?}, second {second:?}"
                );
            }
        }
    }
}
