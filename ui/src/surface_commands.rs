use crate::surfaces::taskbar_diagnostics;
use crate::{app::Application, distribution, model::SurfaceKind, native, texture};
use anyhow::{Context as _, Result};
use gpui_kit::*;
use serde_json::{Value, json};
use std::time::Duration;
impl Application {
    pub(crate) fn surface_command(&mut self, v: &Value, cx: &mut App) -> Result<Value> {
        let op = v["op"].as_str().context("missing op")?;
        let id = v["surface"].as_str().unwrap_or("main");
        match op {
            "dismiss_popup" => {
                if id == crate::app_menu::ID {
                    self.close(id, cx)?;
                    return Ok(json!({}));
                }
                if id == crate::dashboard::ID {
                    self.dismiss_dashboard(cx)?;
                    return Ok(json!({}));
                }
                if let Some(s) = self.surfaces.get(id)
                    && s.view.read(cx).kind == SurfaceKind::Popup
                {
                    for item in &s.view.read(cx).publications {
                        let mut p = item;
                        for _ in 0..8 {
                            let Some(parent) = p.surface.parent.as_ref().and_then(|id| {
                                self.publications.iter().find(|v| {
                                    v.instance == p.instance
                                        && v.id == *id
                                        && v.surface.target == peripheral_ui_protocol::Target::Popup
                                })
                            }) else {
                                break;
                            };
                            p = parent;
                        }
                        self.backend.command(json!({"op":"ui_dismiss","instance":p.instance,"surface":p.id,"generation":p.generation,"revision":p.revision}))?;
                    }
                }
                Ok(json!({}))
            }
            "installation" => distribution::report(),
            "installation_report" => {
                let path = distribution::write_report()?;
                native::open_file(&path)?;
                Ok(json!({"path":path}))
            }
            "ping" => Ok(
                json!({"version":env!("CARGO_PKG_VERSION"),"framework":"gpui-kit 0.9.0","icons":"Lucide SVG","transport":"authenticated loopback JSON","hardware_access":false}),
            ),
            "snapshot" => {
                let surfaces=self.surfaces.iter_mut().map(|(id,s)|{
                    let viewport=s.window.update(cx,|_,w,_|format!("{:?} scale {}",w.viewport_size(),w.scale_factor())).ok();
                    let view=s.view.read(cx);
                    let gpu=gpui_windows::frame_bridge::stats(s.hwnd);
                    let bridge=s.bridge.as_mut().map(|b|{
                        if let Ok(Some(exit))=b.child.try_wait()&& (b.stop_event.is_none()||!exit.success()){b.state="failed".into();b.stats.borrow_mut().error=Some(format!("host process exited: {exit}"));gpui_windows::frame_bridge::unobserve(s.hwnd);}
                        let stats=b.stats.borrow();let consumer=std::fs::read(b.dir.join("consumer.json")).ok().and_then(|d|serde_json::from_slice::<Value>(&d).ok());if let Some(h)=consumer.as_ref().and_then(|c|c["hwnd"].as_u64()){native::post(h as usize,texture::FRAME_MESSAGE+1,0,0);}
                        let diagnostics=taskbar_diagnostics(&b.dir);
                        json!({"state":b.state,"directory":b.dir,"diagnostics":diagnostics,"published":stats.published,"dropped":stats.dropped,"gpu_copy_bytes":stats.gpu_copy_bytes,"cpu_pixel_copies":0,"error":stats.error,"consumer":consumer})});
                    json!({"id":id,"kind":view.kind,"viewport":viewport,"passive":view.passive,"publications":view.publications,"renders":view.renders,"render":{"frames":gpu.frames,"cpu_submit_us":gpu.cpu_submit_us},"native":native::info(s.hwnd),"bridge":bridge})
                }).collect::<Vec<_>>();
                Ok(
                    json!({"notice":self.surfaces.get("main").and_then(|s|s.view.read(cx).manager.as_ref()).and_then(|m|m.notice.as_ref()).map(|n|json!({"revision":n.revision,"kind":n.kind,"message":n.message})),"permission_editor":self.surfaces.get("main").and_then(|s|s.view.read(cx).manager.as_ref()).and_then(|m|m.permission_editor.as_ref()).map(|e|json!({"plugin":e.id,"description":e.description,"selected":e.selected,"saving":e.saving})),"appearance":crate::appearance::options(),"environment":crate::appearance::environment(),"material_available":crate::appearance::material_enabled(),"dashboard_settings":self.dashboard_options,"page":self.surfaces.get("main").and_then(|s|s.view.read(cx).manager.as_ref()).map(|m|&m.page),"configuration":self.surfaces.get("main").and_then(|s|s.view.read(cx).manager.as_ref()).and_then(|m|m.plugin_config.as_ref()).map(|e|json!({"plugin":e.id,"schema":e.schema,"fields":e.fields.iter().map(|f|&f.key).collect::<Vec<_>>(),"errors":e.errors,"saving":e.saving})),"backend":self.surfaces.get("main").and_then(|s|s.view.read(cx).manager.as_ref()).map(|m|&m.backend),"shell":self.shell,"surfaces":surfaces,"foreground_hwnd":native::foreground(),"compositor":native::compositor_timing()}),
                )
            }
            "bridge_ready" => {
                let s = self.surfaces.get_mut(id).context("source closed")?;
                let bridge = s.bridge.as_mut().context("bridge missing")?;
                anyhow::ensure!(
                    bridge.state == "starting",
                    "bridge no longer accepting initialization"
                );
                anyhow::ensure!(
                    v["directory"]
                        .as_str()
                        .is_some_and(|d| std::path::Path::new(d) == bridge.dir),
                    "stale bridge"
                );
                if let Some(error) = v["error"].as_str() {
                    bridge.state = "failed".into();
                    bridge.stats.borrow_mut().error = Some(error.into());
                    anyhow::bail!("host initialization: {error}");
                }
                let h = v["hwnd"].as_u64().context("consumer HWND")? as usize;
                anyhow::ensure!(native::valid_window(h), "consumer window unavailable");
                bridge.state = "ready".into();
                let mut exporter =
                    texture::Exporter::new(h, s.hwnd, bridge.dir.clone(), bridge.stats.clone());
                gpui_windows::frame_bridge::observe(s.hwnd, move |frame| exporter.publish(frame));
                s.view.update(cx, |_, cx| cx.notify());
                Ok(json!({}))
            }
            "bridge_failed" => {
                let s = self.surfaces.get_mut(id).context("source closed")?;
                let b = s.bridge.as_mut().context("bridge missing")?;
                anyhow::ensure!(
                    v["directory"]
                        .as_str()
                        .is_some_and(|d| std::path::Path::new(d) == b.dir),
                    "stale bridge"
                );
                b.state = "failed".into();
                let exit = b
                    .child
                    .try_wait()?
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "still running".into());
                let diagnostics = taskbar_diagnostics(&b.dir);
                let stage = diagnostics
                    .last()
                    .and_then(|v| v["stage"].as_str())
                    .unwrap_or("no native acknowledgement");
                let error = format!(
                    "host did not become ready within {} ms ({exit}); last stage: {stage}",
                    b.startup_timeout_ms
                );
                b.stats.borrow_mut().error = Some(error.clone());
                if let Some(event) = b.stop_event {
                    unsafe {
                        let _ = windows::Win32::System::Threading::SetEvent(event);
                    }
                }
                if b.stop_event.is_none() {
                    let _ = b.child.kill();
                }
                anyhow::bail!(error)
            }
            "pointer" => {
                let s = self.surfaces.get(id).context("surface not found")?;
                if let Some(directory) = v["directory"].as_str() {
                    anyhow::ensure!(
                        s.bridge
                            .as_ref()
                            .is_some_and(|b| b.dir == std::path::Path::new(directory)),
                        "stale bridge input"
                    );
                }
                let event = &v["event"];
                let message = event["message"].as_u64().context("message")? as u32;
                let x = event["x"].as_f64().context("x")? as f32;
                let y = event["y"].as_f64().context("y")? as f32;
                anyhow::ensure!(
                    x.is_finite() && y.is_finite() && x.abs() < 16384. && y.abs() < 16384.,
                    "invalid pointer"
                );
                s.window.update(cx, |_, window, cx| {
                    let position =
                        point(px(x / window.scale_factor()), px(y / window.scale_factor()));
                    let button = if message == 0x204 || message == 0x205 {
                        MouseButton::Right
                    } else {
                        MouseButton::Left
                    };
                    let input = match message {
                        0x201 | 0x204 => Some(PlatformInput::MouseDown(MouseDownEvent {
                            button,
                            position,
                            modifiers: Modifiers::default(),
                            click_count: 1,
                            first_mouse: false,
                        })),
                        0x202 | 0x205 => Some(PlatformInput::MouseUp(MouseUpEvent {
                            button,
                            position,
                            modifiers: Modifiers::default(),
                            click_count: 1,
                        })),
                        0x200 => Some(PlatformInput::MouseMove(MouseMoveEvent {
                            position,
                            pressed_button: if event["wparam"].as_u64().unwrap_or(0) & 2 != 0 {
                                Some(MouseButton::Right)
                            } else if event["wparam"].as_u64().unwrap_or(0) & 1 != 0 {
                                Some(MouseButton::Left)
                            } else {
                                None
                            },
                            modifiers: Modifiers::default(),
                        })),
                        0x20a => Some(PlatformInput::Scroll(ScrollEvent {
                            position,
                            delta: ScrollDelta::Lines(point(
                                0.,
                                ((event["wparam"].as_u64().unwrap_or(0) >> 16) as u16 as i16)
                                    as f32
                                    / 120.,
                            )),
                            ..Default::default()
                        })),
                        _ => None,
                    };
                    if let Some(input) = input {
                        window.dispatch_event(input, cx);
                    }
                })?;
                Ok(json!({}))
            }
            "close" => {
                anyhow::ensure!(id != "main", "use shutdown for main");
                self.close(id, cx)?;
                Ok(json!({}))
            }
            "close_all" => {
                let ids = self
                    .surfaces
                    .keys()
                    .filter(|id| *id != "main")
                    .cloned()
                    .collect::<Vec<_>>();
                for id in ids {
                    self.close(&id, cx)?;
                }
                Ok(json!({}))
            }
            "show" | "hide" => {
                let s = self.surfaces.get_mut(id).context("surface not found")?;
                native::show(s.hwnd, op == "show");
                Ok(json!({}))
            }
            "place" => {
                let s = self.surfaces.get(id).context("surface not found")?;
                let get = |key: &str| {
                    v[key]
                        .as_i64()
                        .and_then(|x| i32::try_from(x).ok())
                        .context("invalid bounds")
                };
                let exported = matches!(
                    s.view.read(cx).kind,
                    SurfaceKind::Texture | SurfaceKind::Xaml | SurfaceKind::Taskbar
                );
                let (x, y) = (get("x")?, get("y")?);
                native::place(
                    s.hwnd,
                    if exported { -10000 } else { x },
                    if exported { -10000 } else { y },
                    get("width")?,
                    get("height")?,
                )?;
                s.window.update(cx, |_, window, _| window.refresh())?;
                Ok(json!({}))
            }
            "toggle_overlay" | "set_passive" => {
                let s = self.surfaces.get(id).context("open overlay first")?;
                let passive = if op == "toggle_overlay" {
                    !s.view.read(cx).passive
                } else {
                    v["passive"].as_bool().context("passive must be bool")?
                };
                native::set_passive(s.hwnd, passive)?;
                s.view.update(cx, |view, cx| {
                    view.passive = passive;
                    cx.notify();
                });
                Ok(json!({"passive":passive}))
            }
            "capture" => {
                let s = self.surfaces.get(id).context("surface not found")?;
                let image = s.window.update(cx, |_, window, cx| {
                    window.refresh();
                    let _ = window.draw(cx);
                    window.render_to_image()
                })??;
                let dir = std::path::PathBuf::from(std::env::var("LOCALAPPDATA")?)
                    .join("peripheral-repl/ui/captures");
                std::fs::create_dir_all(&dir)?;
                let path = dir.join(format!("{id}.png"));
                image.save(&path)?;
                Ok(
                    json!({"path":path,"width":image.width(),"height":image.height(),"cpu_readback":"explicit capture only"}),
                )
            }
            "refresh_preview" => {
                let s = self.surfaces.get_mut(id).context("surface not found")?;
                anyhow::ensure!(
                    s.view.read(cx).kind == SurfaceKind::TaskbarPreview,
                    "not a preview surface"
                );
                if s.preview
                    .as_ref()
                    .is_none_or(|(t, _)| t.elapsed() > Duration::from_millis(500))
                {
                    let image = s.window.update(cx, |_, window, cx| {
                        window.refresh();
                        let _ = window.draw(cx);
                        window.render_to_image()
                    })??;
                    native::cache_preview(s.hwnd, image.clone())?;
                    s.preview = Some((std::time::Instant::now(), image));
                }
                Ok(json!({}))
            }
            "shutdown" => {
                let ids = self.surfaces.keys().cloned().collect::<Vec<_>>();
                for id in ids {
                    self.close(&id, cx)?;
                }
                cx.quit();
                Ok(json!({}))
            }
            _ => anyhow::bail!("unsupported command: {op}"),
        }
    }
}
