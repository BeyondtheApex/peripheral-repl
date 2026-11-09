use crate::{
    app::Application, control, distribution, model::SurfaceKind, native, texture, view::SurfaceView,
};
use anyhow::{Context as _, Result};
use gpui_kit::*;
use serde_json::{Value, json};
use std::time::Duration;
pub(crate) struct Bridge {
    pub(crate) child: std::process::Child,
    pub(crate) dir: std::path::PathBuf,
    pub(crate) stats: std::rc::Rc<std::cell::RefCell<texture::Stats>>,
    pub(crate) stop_event: Option<windows::Win32::Foundation::HANDLE>,
    pub(crate) state: String,
    pub(crate) startup_timeout_ms: u64,
}
impl Drop for Bridge {
    fn drop(&mut self) {
        if let Some(event) = self.stop_event.take() {
            unsafe {
                let _ = windows::Win32::System::Threading::SetEvent(event);
                let _ = windows::Win32::Foundation::CloseHandle(event);
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
pub(crate) struct Surface {
    pub(crate) window: AnyWindowHandle,
    pub(crate) view: Entity<SurfaceView>,
    pub(crate) hwnd: usize,
    pub(crate) preview: Option<(std::time::Instant, image::RgbaImage)>,
    pub(crate) bridge: Option<Bridge>,
}
impl Application {
    pub(crate) fn open(
        &mut self,
        id: &str,
        kind: SurfaceKind,
        dimensions: (f32, f32),
        cx: &mut App,
    ) -> Result<()> {
        anyhow::ensure!(
            !id.is_empty()
                && id.len() <= 48
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
            "invalid surface id"
        );
        if let Some(surface) = self.surfaces.get(id) {
            if surface.view.read(cx).kind == SurfaceKind::Popup {
                native::popup::show_at(surface.hwnd, None)?;
            } else {
                native::show(surface.hwnd, true);
            }
            return Ok(());
        }
        anyhow::ensure!(self.surfaces.len() < 66, "surface limit");
        anyhow::ensure!(
            kind != SurfaceKind::Taskbar
                || !self
                    .surfaces
                    .values()
                    .any(|s| s.view.read(cx).kind == SurfaceKind::Taskbar),
            "only one taskbar bridge may be attached"
        );
        let overlay = kind == SurfaceKind::Overlay;
        let exported = matches!(
            kind,
            SurfaceKind::Taskbar | SurfaceKind::Texture | SurfaceKind::Xaml | SurfaceKind::Tray
        );
        let floating = matches!(
            kind,
            SurfaceKind::Overlay
                | SurfaceKind::Popup
                | SurfaceKind::Taskbar
                | SurfaceKind::Texture
                | SurfaceKind::Xaml
                | SurfaceKind::Tray
        );
        let bounds = if id == "main" {
            Bounds::centered(None, size(px(1120.), px(780.)), cx)
        } else if exported {
            Bounds::new(
                point(px(-10000.), px(-10000.)),
                size(px(dimensions.0), px(dimensions.1)),
            )
        } else if overlay {
            Bounds::new(
                point(px(70.), px(90.)),
                size(px(dimensions.0), px(dimensions.1)),
            )
        } else {
            Bounds::centered(None, size(px(dimensions.0), px(dimensions.1)), cx)
        };
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: if floating {
                None
            } else {
                Some(TitlebarOptions {
                    title: Some(SharedString::from(if id == "main" {
                        crate::i18n::tr("Peripheral · 插件管理")
                    } else {
                        format!("Peripheral · {id}")
                    })),
                    appears_transparent: false,
                    ..Default::default()
                })
            },
            focus: !floating,
            show: true,
            kind: if floating {
                WindowKind::PopUp
            } else {
                WindowKind::Normal
            },
            is_resizable: !floating,
            is_movable: !floating,
            is_minimizable: !floating,
            window_background: if id == "main" || kind == SurfaceKind::Popup {
                crate::appearance::window_background(kind == SurfaceKind::Popup)
            } else if floating {
                WindowBackgroundAppearance::Transparent
            } else {
                WindowBackgroundAppearance::Opaque
            },
            window_min_size: Some(if id == "main" {
                size(px(1000.), px(700.))
            } else {
                size(px(16.), px(16.))
            }),
            ..Default::default()
        };
        let commands = self.commands.clone();
        let view_id = id.to_owned();
        let mut hwnd = 0;
        let mut built = None;
        let window = cx.open_window(options, |window, cx| {
            hwnd = native::hwnd(window).expect("native HWND");
            let view = cx.new(|cx| SurfaceView::new(view_id, kind, commands, window, cx));
            built = Some(view.clone());
            cx.new(|cx| {
                let root = base::Root::new(view, window, cx);
                // Kit's theme plugin paints an opaque root by default. A transparent
                // swapchain alone cannot expose the desktop through rounded corners.
                if floating || id == "main" {
                    root.bg(transparent_black())
                } else {
                    root
                }
            })
        })?;
        let window: AnyWindowHandle = window.into();
        let view = built.context("window content was not created")?;
        native::install(hwnd, id.to_owned(), overlay, self.commands.clone())?;
        if floating {
            native::floating_frame(hwnd, true)?;
        }
        native::appearance::apply(
            hwnd,
            id == "main",
            kind == SurfaceKind::Popup,
            &crate::appearance::options(),
        );
        self.surfaces.insert(
            id.into(),
            Surface {
                window,
                view,
                hwnd,
                preview: None,
                bridge: None,
            },
        );
        if kind == SurfaceKind::TaskbarPreview {
            native::enable_preview(hwnd)?;
        }
        let commands = self.commands.clone();
        let close_id = id.to_owned();
        window.update(cx, move |_, window, cx| {
            window.on_window_should_close(cx, move |_, _| {
                let (tx, _) = std::sync::mpsc::sync_channel(1);
                let _ = commands.try_send(control::Request {
                    body: json!({"op":if close_id=="main"{"hide"}else{"close"},"surface":close_id}),
                    reply: tx,
                    expires: None,
                });
                false
            })
        })?;
        if exported {
            let scale = window.update(cx, |_, window, _| window.scale_factor())?;
            native::prepare_export(
                hwnd,
                (dimensions.0 * scale).round() as i32,
                (dimensions.1 * scale).round() as i32,
            )?;
            gpui_windows::frame_bridge::external_only(hwnd);
        } else {
            native::show(hwnd, true);
        }
        if id == "main" {
            native::add_tray(hwnd, 1, "icons/cpu.svg", cx)?;
        }
        Ok(())
    }
    pub(crate) fn close(&mut self, id: &str, cx: &mut App) -> Result<()> {
        let surface = self.surfaces.remove(id).context("surface not found")?;
        gpui_windows::frame_bridge::remove(surface.hwnd);
        surface
            .window
            .update(cx, |_, window, _| window.remove_window())?;
        Ok(())
    }
    pub(crate) fn start_bridge(&mut self, id: &str, cx: &mut App) -> Result<()> {
        use std::os::windows::process::CommandExt;
        let s = self.surfaces.get_mut(id).context("surface not found")?;
        anyhow::ensure!(s.bridge.is_none(), "bridge already attached");
        let dir = std::path::PathBuf::from(std::env::var("LOCALAPPDATA")?)
            .join("peripheral-repl/ui/bridges")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&dir)?;
        // Packaged launchers can virtualize LOCALAPPDATA. Explorer sees the
        // physical filesystem, so every path crossing into it must be resolved.
        let dir = std::fs::canonicalize(&dir).context("resolve bridge physical directory")?;
        let startup_timeout_ms = if s.view.read(cx).kind == SurfaceKind::Taskbar {
            12000
        } else {
            3000
        };
        let mut stop_event = None;
        let child = if s.view.read(cx).kind == SurfaceKind::Taskbar {
            let (child, event) = start_taskbar(&dir)?;
            stop_event = Some(event);
            child
        } else if s.view.read(cx).kind == SurfaceKind::Xaml {
            distribution::verified_dll()?;
            std::process::Command::new(std::env::current_exe()?)
                .arg("--xaml-consumer")
                .arg(&dir)
                .creation_flags(0x08000000)
                .spawn()?
        } else {
            std::process::Command::new(std::env::current_exe()?)
                .arg("--texture-consumer")
                .arg(&dir)
                .arg(id)
                .creation_flags(0x08000000)
                .spawn()?
        };
        s.bridge = Some(Bridge {
            child,
            dir: dir.clone(),
            stats: Default::default(),
            stop_event,
            state: "starting".into(),
            startup_timeout_ms,
        });
        let commands = self.commands.clone();
        let id = id.to_owned();
        cx.spawn(async move|cx|{
            for _ in 0..startup_timeout_ms/20 {
                cx.background_executor().timer(Duration::from_millis(20)).await;
                if let Ok(data)=std::fs::read(dir.join("consumer.json"))
                    && let Ok(value)=serde_json::from_slice::<Value>(&data){let(tx,_)=std::sync::mpsc::sync_channel(1);let _=commands.try_send(control::Request{body:json!({"op":"bridge_ready","surface":id,"hwnd":value["hwnd"],"directory":dir,"error":value["error"]}),reply:tx,expires:None});return;}
            }
            let(tx,_)=std::sync::mpsc::sync_channel(1);let _=commands.try_send(control::Request{body:json!({"op":"bridge_failed","surface":id,"directory":dir}),reply:tx,expires:None});
        }).detach();
        Ok(())
    }
}
fn start_taskbar(
    dir: &std::path::Path,
) -> Result<(std::process::Child, windows::Win32::Foundation::HANDLE)> {
    use std::os::windows::process::CommandExt;
    use windows::{
        Win32::{
            System::Threading::CreateEventW,
            UI::WindowsAndMessaging::{FindWindowW, GetWindowThreadProcessId},
        },
        core::{HSTRING, w},
    };
    let hwnd = unsafe { FindWindowW(w!("Shell_TrayWnd"), None)? };
    let mut pid = 0;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    anyhow::ensure!(pid != 0, "Explorer taskbar unavailable");
    let stop_name = format!("Local\\PeripheralTaskbarStop_{}", uuid::Uuid::new_v4());
    let event = unsafe { CreateEventW(None, true, false, &HSTRING::from(&stop_name))? };
    let result = (|| -> Result<std::process::Child> {
        let dll = distribution::verified_dll()?;
        let config = json!({"protocol":distribution::ABI,"native_build":distribution::expected_build(),"native_dll":dll,"producer_pid":std::process::id(),"explorer_pid":pid,"directory":dir,"stop_event":stop_name});
        std::fs::write(
            dir.join("taskbar.json"),
            serde_json::to_vec_pretty(&config)?,
        )?;
        Ok(std::process::Command::new(std::env::current_exe()?)
            .arg("--taskbar-loader")
            .arg(pid.to_string())
            .arg(dir.join("taskbar.json"))
            .creation_flags(0x08000000)
            .spawn()?)
    })();
    match result {
        Ok(child) => Ok((child, event)),
        Err(e) => {
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(event);
            }
            Err(e)
        }
    }
}

pub(crate) fn taskbar_diagnostics(dir: &std::path::Path) -> Vec<Value> {
    use std::io::{BufRead, Read};
    let Ok(file) = std::fs::File::open(dir.join("taskbar-trace.jsonl")) else {
        return Vec::new();
    };
    std::io::BufReader::new(file)
        .take(131072)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str(&line).ok())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .take(24)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}
