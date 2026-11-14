#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod ai_setup;
mod app;
mod app_menu;
mod appearance;
mod backend;
mod backend_control;
mod backend_status;
mod clipboard;
mod control;
mod dashboard;
mod device_updates;
mod distribution;
mod i18n;
mod management_commands;
mod manager;
mod model;
mod native;
mod notifications;
mod permissions;
mod plugin_commands;
mod presentation;
mod publications;
mod shell;
mod startup;
mod surface_commands;
mod surfaces;
mod taskbar;
mod texture;
mod updates;
mod view;
use anyhow::{Context as _, Result};
use model::SurfaceKind;
use serde_json::{Value, json};
fn main() -> Result<()> {
    let args = std::env::args().collect::<Vec<_>>();
    if let Some(mode) = args.get(1) {
        if mode == "--update-report" {
            anyhow::ensure!(
                args.len() <= 3,
                "usage: peripheral-ui.exe --update-report [installation-directory]"
            );
            let root = args
                .get(2)
                .map(std::path::PathBuf::from)
                .map(Ok)
                .unwrap_or_else(updates::root)?;
            updates::write_report(&root)?;
            return Ok(());
        }
        if ["--xaml-consumer", "--taskbar-loader", "--native-check"].contains(&mode.as_str()) {
            match distribution::run_helper(mode, &args[2..]) {
                Ok(code) => std::process::exit(code),
                Err(error) => {
                    eprintln!("native helper: {error:#}");
                    std::process::exit(21);
                }
            }
        }
        if mode == "--installation-report" {
            println!("{}", distribution::write_report()?.display());
            return Ok(());
        }
    }
    if args.get(1).is_some_and(|s| s == "--texture-consumer") {
        return texture::run_consumer(
            std::path::Path::new(args.get(2).context("bridge directory")?),
            args.get(3).context("source id")?,
        );
    }
    if let Some(mode) = args.get(1) {
        match mode.as_str() {
            "--mcp" => {
                anyhow::ensure!(
                    args.len() == 3,
                    "usage: peripheral-ui.exe --mcp <endpoint-file>"
                );
                return peripheral_repl::protocol::mcp_bridge(std::path::PathBuf::from(&args[2]));
            }
            "--native-hid-worker" => return peripheral_repl::native_hid::serve(),
            "--daemon-elevated" => return backend::run_elevated(&args[2..]),
            "--elevate-backend" => {
                anyhow::ensure!(
                    args.len() == 4,
                    "usage: --elevate-backend <workspace> <endpoint-file>"
                );
                return backend::elevate(
                    std::path::Path::new(&args[3]),
                    std::path::Path::new(&args[2]),
                );
            }
            "--daemon" => {
                return backend::run_daemon(std::path::Path::new(
                    args.get(2).context("workspace path")?,
                ));
            }
            "--restart-shell" => {
                return updates::restart_shell(args.get(2).is_some_and(|a| a == "--allow-stop"));
            }
            "--apply-update" => return updates::apply(&args[2..]),
            _ => {}
        }
    }
    let autostart = args.get(1).is_some_and(|s| s == "--autostart");
    let resume_backend = match args.get(1).map(String::as_str) {
        Some("--resume-backend") => backend::Resume::Normal,
        Some("--resume-backend-elevated") => backend::Resume::Elevated,
        _ => backend::Resume::Stopped,
    };
    let _singleton = native::singleton()?;
    let update_recovery_error = updates::root()
        .and_then(|root| updates::reconcile_pending(&root))
        .err()
        .map(|error| format!("Update state check failed: {error:#}"));
    let (sender, receiver, session) = control::start()?;
    let _ = distribution::write_report();
    eprintln!("UI control: {}", session.display());
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            appearance::apply(&appearance::Options::load(), cx);
            let mut application =
                app::Application::new(sender, autostart && startup::state().start_in_tray);
            if let Err(e) = application.open("main", SurfaceKind::Window, (1120., 780.), cx) {
                eprintln!("startup: {e:#}");
                cx.quit();
                return;
            }
            application.start_watch();
            if let Some(error) = &update_recovery_error {
                application.notify("error", error.clone(), cx);
            }
            if let Ok(root) = updates::root()
                && let Some(result) = updates::result(&root)
                && matches!(
                    result["state"].as_str(),
                    Some("error" | "applied_shell_error" | "applied_ui_error")
                )
            {
                application.notify(
                    "error",
                    result["message"]
                        .as_str()
                        .unwrap_or("Update failed; see updates/result.json"),
                    cx,
                );
            }
            if let Some(action) = resume_backend.action().or(autostart.then_some("start"))
                && let Err(error) = application.control_backend(action, cx)
            {
                application.notify("error", format!("{error:#}"), cx);
                if let Some(main) = application.surfaces.get("main") {
                    native::show(main.hwnd, true);
                }
            }
            cx.spawn(async move |cx| {
                while let Ok(request) = receiver.recv().await {
                    if request
                        .expires
                        .is_some_and(|deadline| std::time::Instant::now() > deadline)
                    {
                        let _ = request
                            .reply
                            .send(json!({"ok":false,"error":"request expired"}));
                        continue;
                    }
                    let result = cx.update(|cx| {
                        let result = application.command(&request.body, cx);
                        if let Err(error) = &result {
                            application.notify("error", format!("{error:#}"), cx);
                        }
                        match result {
                            Ok(result) => json!({"ok":true,"result":result}),
                            Err(e) => json!({"ok":false,"error":format!("{e:#}")}),
                        }
                    });
                    let _ = request.reply.send(result);
                }
            })
            .detach();
        });
    if std::fs::read_to_string(&session)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .is_some_and(|v| v["pid"] == std::process::id())
    {
        let _ = std::fs::remove_file(session);
    }
    Ok(())
}
