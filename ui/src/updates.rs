//! Fixed staging location and recoverable two-file replacement. Never patch a
//! mapped module. Shell restarts run in a bounded, explicitly requested helper.
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::{
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        System::{Diagnostics::ToolHelp::*, Threading::*},
        UI::WindowsAndMessaging::*,
    },
    core::{HSTRING, w},
};

mod explorer;
mod operation;
mod package;
mod recovery;
mod relaunch;
mod report;
mod runner;
use explorer::ShellProcess;
pub(crate) use explorer::process_image;
pub use explorer::{explorer_pid, loaded_modules};
use package::replace_pair;
pub use package::{stage, verify_package};
pub use recovery::reconcile as reconcile_pending;
pub use report::write as write_report;

const FILES: [&str; 2] = ["peripheral-ui.exe", "peripheral-taskbar-hook.dll"];
pub fn record_preparation(root: &Path) -> Result<()> {
    let _guard = operation::Guard::acquire(root)?;
    operation::progress(
        root,
        "preparing",
        "Checking and stopping this installation's backend before update",
    )
}
pub fn record_preparation_error(root: &Path, error: &anyhow::Error) {
    if let Ok(_guard) = operation::Guard::acquire(root) {
        let _ = operation::progress(
            root,
            "error",
            &format!("Update preparation failed; UI kept open: {error:#}"),
        );
    }
}
pub fn root() -> Result<PathBuf> {
    Ok(std::env::current_exe()?
        .parent()
        .context("installation folder")?
        .to_owned())
}
fn atomic_json(path: &Path, value: &Value) -> Result<()> {
    let tmp = path.with_extension("new");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    // MoveFileEx replaces an existing status atomically; readers see complete JSON.
    unsafe {
        windows::Win32::Storage::FileSystem::MoveFileExW(
            &windows::core::HSTRING::from(tmp.as_os_str()),
            &windows::core::HSTRING::from(path.as_os_str()),
            windows::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING
                | windows::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
        )?;
    }
    Ok(())
}
pub fn pending(root: &Path) -> Option<Value> {
    std::fs::read(root.join("updates/pending/update.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .filter(|v| v["state"] == "ready")
}
pub fn result(root: &Path) -> Option<Value> {
    std::fs::read(root.join("updates/result.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
}
pub fn launch_apply(
    restart: bool,
    mount: bool,
    resume_backend: crate::backend::Resume,
) -> Result<()> {
    let root = root()?;
    // Clear a historical success before any operation that may fail to spawn.
    // Do not overwrite another worker's status if it holds the installation lock.
    {
        let _guard = operation::Guard::acquire(&root)?;
        operation::progress(
            &root,
            "starting",
            "Preparing the current-version update helper",
        )?;
    }
    let outcome = launch_apply_inner(&root, restart, mount, resume_backend);
    if let Err(error) = &outcome
        && let Ok(_guard) = operation::Guard::acquire(&root)
    {
        operation::progress(
            &root,
            "error",
            &format!("Updater startup failed: {error:#}"),
        )?;
    }
    outcome
}
fn launch_apply_inner(
    root: &Path,
    restart: bool,
    mount: bool,
    resume_backend: crate::backend::Resume,
) -> Result<()> {
    anyhow::ensure!(pending(root).is_some(), "no prepared update");
    verify_package(&root.join("updates/pending"))?;
    let ticket = uuid::Uuid::new_v4().to_string();
    let ready = root.join("updates").join(format!("ready-{ticket}.json"));
    let executable = runner::prepare(root, &ticket)?;
    let mut child = Command::new(executable)
        .args([
            "--apply-update",
            &std::process::id().to_string(),
            if restart { "restart-confirmed" } else { "wait" },
            if mount { "mount" } else { "no-mount" },
            resume_backend.update_argument(),
            &ticket,
        ])
        .creation_flags(0x08000000)
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if ready.exists() {
            let _ = std::fs::remove_file(ready);
            return Ok(());
        }
        if let Some(exit) = child.try_wait()? {
            anyhow::bail!(
                "Updater exited before accepting the update: {exit}; {}",
                result(root).unwrap_or(Value::Null)
            );
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("Updater did not acknowledge startup; the UI has been kept open");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
pub fn launch_restart() -> Result<()> {
    let mut child = Command::new(std::env::current_exe()?)
        .args(["--restart-shell", "--allow-stop"])
        .creation_flags(0x08000000)
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(exit) = child.try_wait()? {
            anyhow::ensure!(
                exit.success(),
                "{}",
                result(&root()?)
                    .and_then(|v| v["message"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| format!("Explorer restart helper failed: {exit}"))
            );
            return Ok(());
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            explorer::ensure_shell_running()?;
            anyhow::bail!("Explorer restart helper exceeded 30 seconds; operation cancelled");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
pub fn restart_shell(allow_stop: bool) -> Result<()> {
    let root = root()?;
    let _guard = operation::Guard::acquire(&root)?;
    operation::progress(
        &root,
        "restarting",
        "Restarting the primary Explorer process (bounded timeout)",
    )?;
    let result = ShellProcess::capture()
        .and_then(|s| s.stop(allow_stop))
        .and_then(|mut s| s.resume());
    operation::progress(
        &root,
        if result.is_ok() { "restarted" } else { "error" },
        &result
            .as_ref()
            .map(|_| "Explorer restarted".to_owned())
            .unwrap_or_else(|e| format!("{e:#}")),
    )?;
    result
}

pub fn apply(args: &[String]) -> Result<()> {
    let installation = runner::installation(&root()?)?;
    let updates = installation.join("updates");
    let pending = updates.join("pending");
    let pid: u32 = args.first().context("UI PID")?.parse()?;
    let allow_stop = args.get(1).is_some_and(|v| v == "restart-confirmed");
    let restart = allow_stop || args.get(1).is_some_and(|v| v == "restart");
    let resume_backend =
        crate::backend::Resume::from_update_argument(args.get(3).map(String::as_str))?;
    let _guard = operation::Guard::acquire(&installation)?;
    let mut installed = false;
    operation::progress(
        &installation,
        "preflight",
        "Checking package and restart prerequisites",
    )?;
    let parent = runner::ParentProcess::capture(pid).inspect_err(|error| {
        let _ = operation::progress(
            &installation,
            "error",
            &format!("Cannot capture old UI process: {error:#}"),
        );
    })?;
    let outcome = (|| -> Result<()> {
        verify_package(&pending)?;
        anyhow::ensure!(
            self::pending(&installation).is_some(),
            "no ready update marker; partial staging will not be applied"
        );
        // Complete restart prerequisites before acknowledging UI shutdown.
        let captured_shell = if restart && explorer::installation_loaded(&installation)? {
            Some(ShellProcess::capture()?)
        } else {
            None
        };
        operation::progress(
            &installation,
            "waiting_ui",
            "Waiting for the old UI to exit",
        )?;
        if let Some(ticket) = args.get(4) {
            let ticket = uuid::Uuid::parse_str(ticket)?;
            atomic_json(
                &updates.join(format!("ready-{ticket}.json")),
                &json!({"pid":std::process::id()}),
            )?;
        }
        parent.wait()?;
        let mut shell = if let Some(captured) = captured_shell {
            operation::progress(
                &installation,
                "restarting",
                "Stopping the primary Explorer process",
            )?;
            Some(captured.stop(allow_stop)?)
        } else {
            None
        };
        let replacement = (|| -> Result<()> {
            operation::progress(
                &installation,
                "installing",
                "Installing and verifying the matching EXE/DLL pair",
            )?;
            if let Some(pid) = explorer_pid() {
                let mapped = loaded_modules(pid)?;
                let dll = std::fs::canonicalize(installation.join(FILES[1]))?;
                anyhow::ensure!(
                    !mapped
                        .iter()
                        .any(|p| std::fs::canonicalize(p).ok().as_ref() == Some(&dll)),
                    "Explorer still maps the old DLL. The update is retained; use Restart and update to release it"
                );
            }
            replace_pair(&pending, &installation)?;
            installed = true;
            // Mark committed before restoring Explorer. A shell restart failure
            // must not cause this package to be applied a second time.
            atomic_json(&pending.join("update.json"), &json!({"state":"applied"}))?;
            let _ = std::fs::remove_file(pending.join("update.json"));
            Ok(())
        })();
        let resumed = shell.as_mut().map(|s| s.resume()).transpose();
        replacement?;
        resumed?;
        Ok(())
    })();
    let state = if outcome.is_ok() {
        "applied"
    } else if installed {
        "applied_shell_error"
    } else {
        "error"
    };
    let message = outcome
        .as_ref()
        .map(|_| {
            "Matching EXE/DLL installed and verified; previous version retained in updates/previous"
                .to_owned()
        })
        .unwrap_or_else(|e| format!("{e:#}"));
    let reopening = parent.exited();
    let status_result = operation::progress(
        &installation,
        if reopening && outcome.is_ok() {
            "reopening_ui"
        } else {
            state
        },
        &message,
    );
    // Recovery on the relaunched UI also acquires this lock. Release it before
    // creating that process, including when the update failed and remains ready.
    drop(_guard);
    // A preflight failure while the old UI is alive must not launch a duplicate.
    if reopening && let Err(error) = relaunch::start(&installation, resume_backend) {
        operation::progress(
            &installation,
            if installed {
                "applied_ui_error"
            } else {
                "error"
            },
            &format!(
                "{}; could not confirm UI startup for {}: {error:#}",
                outcome
                    .as_ref()
                    .map(|_| "Files installed and verified".to_owned())
                    .unwrap_or_else(|e| format!("{e:#}")),
                installation.join(FILES[0]).display()
            ),
        )?;
        return Err(error);
    }
    status_result?;
    if reopening {
        operation::progress(&installation, state, &message)?;
    }
    outcome
}
