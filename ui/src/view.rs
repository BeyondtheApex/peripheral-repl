use crate::{control::Request, manager::Manager, model::SurfaceKind};
use gpui_kit::*;
use peripheral_repl::ui::Publication;
pub struct SurfaceView {
    pub kind: SurfaceKind,
    pub passive: bool,
    pub publications: Vec<Publication>,
    pub manager: Option<Manager>,
    pub dashboard: Option<crate::dashboard::Dashboard>,
    pub app_menu: Option<crate::backend_status::BackendStatus>,
    pub renders: u64,
    commands: async_channel::Sender<Request>,
}
impl SurfaceView {
    pub fn new(
        id: String,
        kind: SurfaceKind,
        commands: async_channel::Sender<Request>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let manager = (id == "main").then(|| Manager::new(window, cx));
        Self {
            kind,
            passive: kind == SurfaceKind::Overlay,
            publications: Vec::new(),
            manager,
            dashboard: (id == crate::dashboard::ID).then(crate::dashboard::Dashboard::default),
            app_menu: (id == crate::app_menu::ID).then_some(Default::default()),
            renders: 0,
            commands,
        }
    }
}
impl Render for SurfaceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        if let Some(status) = self.app_menu {
            return crate::app_menu::render(status, &self.commands);
        }
        if let Some(manager) = &self.manager {
            return manager.render(&self.commands, cx);
        }
        if let Some(dashboard) = &self.dashboard {
            return dashboard.render(&self.publications, &self.commands, window.viewport_size());
        }
        let viewport = window.viewport_size();
        let total = self
            .publications
            .iter()
            .map(|p| p.surface.width)
            .sum::<f32>()
            .max(1.);
        let fit = (f32::from(viewport.width) / total).min(1.);
        div()
            .size_full()
            .flex()
            .items_center()
            .children(self.publications.iter().map(|p| {
                crate::presentation::document_in(
                    p,
                    self.commands.clone(),
                    p.surface.width * fit,
                    p.surface.height.min(f32::from(viewport.height)),
                )
            }))
            .into_any_element()
    }
}
