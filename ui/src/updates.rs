//! Fixed staging location and recoverable two-file replacement. Never patch a
//! mapped module or force-terminate Explorer. All executable paths are explicit.
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
        System::{Diagnostics::ToolHelp::*, RestartManager::*, Threading::*},
        UI::WindowsAndMessaging::*,
    },
    core::{PWSTR, w},
};

const FILES: [&str; 2] = ["peripheral-ui.exe", "peripheral-taskbar-hook.dll"];
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
pub fn verify_package(folder: &Path) -> Result<Value> {
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(folder.join("release-manifest.json")).context("release manifest missing")?,
    )?;
    anyhow::ensure!(
        manifest["format"] == 1 && manifest["architecture"] == "x64",
        "unsupported package format or architecture"
    );
    let entries = manifest["files"]
        .as_array()
        .context("package file inventory")?;
    anyhow::ensure!(
        entries.len() == 2,
        "package must contain exactly two binary entries"
    );
    for name in FILES {
        let entries = entries
            .iter()
            .filter(|v| v["name"] == name)
            .collect::<Vec<_>>();
        anyhow::ensure!(
            entries.len() == 1,
            "missing or duplicate package file: {name}"
        );
        let expected = entries[0]["sha256"].as_str().context("file hash missing")?;
        anyhow::ensure!(
            crate::distribution::hash(&folder.join(name))? == expected,
            "package file hash mismatch: {name}"
        );
    }
    Ok(manifest)
}
pub fn stage(source: &Path, installation: &Path) -> Result<Value> {
    let manifest = verify_package(source)?;
    let destination = installation.join("updates/pending");
    anyhow::ensure!(
        std::fs::canonicalize(source).ok() != std::fs::canonicalize(&destination).ok()
            || !destination.exists(),
        "select the original release folder, not the pending folder"
    );
    std::fs::create_dir_all(&destination)?;
    // Invalidate readiness first. Partial copies are never applied automatically.
    let marker = destination.join("update.json");
    if marker.exists() {
        std::fs::remove_file(&marker)?;
    }
    for name in FILES {
        std::fs::copy(source.join(name), destination.join(name))
            .with_context(|| format!("stage {name}; previous updater may still be running"))?;
    }
    atomic_json(&destination.join("release-manifest.json"), &manifest)?;
    verify_package(&destination)?;
    let mut child = Command::new(destination.join(FILES[0]))
        .arg("--native-check")
        .creation_flags(0x08000000)
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(8);
    let valid = loop {
        if let Some(code) = child.try_wait()? {
            break code.success();
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("staged native preflight timed out");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    anyhow::ensure!(valid, "staged EXE and DLL do not match");
    let value = json!({"state":"ready","manifest":manifest,"directory":destination,"apply_after_explorer_restart":true});
    atomic_json(&marker, &value)?;
    Ok(value)
}
pub fn explorer_pid() -> Option<u32> {
    unsafe {
        let h = FindWindowW(w!("Shell_TrayWnd"), None).ok()?;
        let mut pid = 0;
        GetWindowThreadProcessId(h, Some(&mut pid));
        (pid != 0).then_some(pid)
    }
}
pub fn loaded_modules(pid: u32) -> Result<Vec<PathBuf>> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid)?;
        let mut entry = MODULEENTRY32W {
            dwSize: std::mem::size_of::<MODULEENTRY32W>() as u32,
            ..Default::default()
        };
        let mut paths = Vec::new();
        let mut result = Module32FirstW(snapshot, &mut entry);
        while result.is_ok() {
            let name = String::from_utf16_lossy(
                &entry.szModule[..entry
                    .szModule
                    .iter()
                    .position(|x| *x == 0)
                    .unwrap_or(entry.szModule.len())],
            );
            if name.eq_ignore_ascii_case(FILES[1]) {
                let end = entry
                    .szExePath
                    .iter()
                    .position(|x| *x == 0)
                    .unwrap_or(entry.szExePath.len());
                paths.push(PathBuf::from(String::from_utf16_lossy(
                    &entry.szExePath[..end],
                )));
            }
            result = Module32NextW(snapshot, &mut entry);
        }
        let _ = CloseHandle(snapshot);
        Ok(paths)
    }
}
pub fn launch_apply(restart: bool, mount: bool) -> Result<()> {
    let root = root()?;
    anyhow::ensure!(pending(&root).is_some(), "no prepared update");
    verify_package(&root.join("updates/pending"))?;
    Command::new(root.join("updates/pending/peripheral-ui.exe"))
        .args([
            "--apply-update",
            &std::process::id().to_string(),
            if restart { "restart" } else { "wait" },
            if mount { "mount" } else { "no-mount" },
        ])
        .creation_flags(0x08000000)
        .spawn()?;
    Ok(())
}
pub fn launch_restart() -> Result<()> {
    Command::new(std::env::current_exe()?)
        .arg("--restart-shell")
        .creation_flags(0x08000000)
        .spawn()?;
    Ok(())
}
struct RestartSession(u32);
impl Drop for RestartSession {
    fn drop(&mut self) {
        unsafe {
            let _ = RmEndSession(self.0);
        }
    }
}
impl RestartSession {
    fn stop_shell() -> Result<Self> {
        unsafe {
            let pid = explorer_pid().context("Explorer taskbar not running")?;
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)?;
            let (mut start, mut exit, mut kernel, mut user) = (
                FILETIME::default(),
                FILETIME::default(),
                FILETIME::default(),
                FILETIME::default(),
            );
            let times = GetProcessTimes(process, &mut start, &mut exit, &mut kernel, &mut user);
            let _ = CloseHandle(process);
            times?;
            let mut id = 0;
            let mut key = [0u16; 33];
            RmStartSession(&mut id, None, PWSTR(key.as_mut_ptr())).ok()?;
            let session = Self(id);
            RmRegisterResources(
                id,
                None,
                Some(&[RM_UNIQUE_PROCESS {
                    dwProcessId: pid,
                    ProcessStartTime: start,
                }]),
                None,
            )
            .ok()?;
            let error = RmShutdown(id, 0, None); // Graceful only. No RmForceShutdown / taskkill.
            if error != ERROR_SUCCESS {
                let _ = RmRestart(id, None, None);
                anyhow::bail!(
                    "Windows declined graceful Explorer restart ({error:?}); restart Explorer manually or sign out. No forced termination was attempted"
                );
            }
            Ok(session)
        }
    }
    fn resume(&self) -> Result<()> {
        unsafe {
            RmRestart(self.0, None, None).ok()?;
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while explorer_pid().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        anyhow::ensure!(
            explorer_pid().is_some(),
            "Explorer restart has not completed; use Task Manager to start explorer.exe"
        );
        Ok(())
    }
}
pub fn restart_shell() -> Result<()> {
    let root = root()?;
    std::fs::create_dir_all(root.join("updates"))?;
    atomic_json(
        &root.join("updates/result.json"),
        &json!({"state":"restarting","message":"Waiting for Windows to restart Explorer gracefully"}),
    )?;
    let result = RestartSession::stop_shell().and_then(|session| session.resume());
    atomic_json(
        &root.join("updates/result.json"),
        &match &result {
            Ok(_) => json!({"state":"restarted","message":"Explorer restarted"}),
            Err(e) => json!({"state":"error","message":format!("{e:#}")}),
        },
    )?;
    result
}
fn replace_pair(pending: &Path, installation: &Path) -> Result<()> {
    verify_package(pending)?;
    let backup = installation.join("updates/previous");
    std::fs::create_dir_all(&backup)?;
    let mut moved = Vec::new();
    let mut installed = Vec::new();
    let result = (|| -> Result<()> {
        for name in FILES {
            let dest = backup.join(name);
            if dest.exists() {
                std::fs::remove_file(&dest)?;
            }
            std::fs::rename(installation.join(name), &dest).with_context(|| {
                format!("{name} is locked; close its users and retry after Explorer exits")
            })?;
            moved.push(name);
        }
        for name in FILES {
            let temp = installation.join(format!("{name}.new"));
            std::fs::copy(pending.join(name), &temp)?;
            std::fs::rename(temp, installation.join(name))?;
            installed.push(name);
        }
        std::fs::copy(
            pending.join("release-manifest.json"),
            installation.join("release-manifest.json"),
        )?;
        Ok(())
    })();
    if let Err(error) = result {
        let mut rollback_errors = Vec::new();
        for name in installed {
            if let Err(e) = std::fs::remove_file(installation.join(name)) {
                rollback_errors.push(e.to_string());
            }
        }
        for name in moved {
            if let Err(e) = std::fs::rename(backup.join(name), installation.join(name)) {
                rollback_errors.push(e.to_string());
            }
        }
        anyhow::bail!("update failed: {error:#}; rollback errors: {rollback_errors:?}");
    }
    Ok(())
}
pub fn apply(args: &[String]) -> Result<()> {
    let pending = root()?;
    anyhow::ensure!(
        pending.file_name().is_some_and(|n| n == "pending"),
        "updater must run from updates/pending"
    );
    let updates = pending.parent().context("updates folder")?;
    anyhow::ensure!(
        updates.file_name().is_some_and(|n| n == "updates"),
        "invalid staging location"
    );
    let installation = updates.parent().context("installation folder")?;
    let pid: u32 = args.first().context("UI PID")?.parse()?;
    let restart = args.get(1).is_some_and(|v| v == "restart");
    let mount = args.get(2).is_some_and(|v| v == "mount");
    let result = (|| -> Result<()> {
        verify_package(&pending)?;
        anyhow::ensure!(
            self::pending(installation).is_some(),
            "no ready update marker; partial staging will not be applied"
        );
        unsafe {
            if let Ok(process) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
                let result = WaitForSingleObject(process, 10000);
                let _ = CloseHandle(process);
                anyhow::ensure!(result == WAIT_OBJECT_0, "UI did not exit; update cancelled");
            }
        }
        let session = if restart {
            Some(RestartSession::stop_shell()?)
        } else {
            None
        };
        // Always attempt to restore the shell, including preflight failures.
        let replacement = (|| -> Result<()> {
            if let Some(pid) = explorer_pid() {
                let mapped = loaded_modules(pid)?;
                let install_dll = std::fs::canonicalize(installation.join(FILES[1]))?;
                anyhow::ensure!(
                    !mapped
                        .iter()
                        .any(|p| std::fs::canonicalize(p).ok().as_ref() == Some(&install_dll)),
                    "Explorer still maps the installed DLL; wait for it to exit before applying"
                );
            }
            replace_pair(&pending, installation)
        })();
        let resumed = session.as_ref().map(|s| s.resume()).transpose();
        replacement?;
        resumed?;
        std::fs::remove_file(pending.join("update.json"))?;
        Ok(())
    })();
    atomic_json(
        &updates.join("result.json"),
        &match &result {
            Ok(_) => {
                json!({"state":"applied","message":"Matching EXE/DLL installed; previous binaries retained in updates/previous"})
            }
            Err(e) => json!({"state":"error","message":format!("{e:#}")}),
        },
    )?;
    let mut command = Command::new(installation.join(FILES[0]));
    if mount && result.is_ok() {
        command.arg("--restore-taskbar");
    }
    command.creation_flags(0x08000000).spawn()?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("peripheral-update-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }
    fn package(path: &Path) {
        let files = FILES
            .iter()
            .map(|name| {
                std::fs::write(path.join(name), format!("new {name}")).unwrap();
                json!({"name":name,"sha256":crate::distribution::hash(&path.join(name)).unwrap()})
            })
            .collect::<Vec<_>>();
        atomic_json(
            &path.join("release-manifest.json"),
            &json!({"format":1,"architecture":"x64","files":files}),
        )
        .unwrap();
    }
    #[test]
    fn replacement_preserves_backup_and_rejects_modified_package() {
        let install = fixture();
        let pending = install.join("updates/pending");
        std::fs::create_dir_all(&pending).unwrap();
        package(&pending);
        for name in FILES {
            std::fs::write(install.join(name), "old").unwrap();
        }
        replace_pair(&pending, &install).unwrap();
        assert_eq!(
            std::fs::read_to_string(install.join("updates/previous/peripheral-ui.exe")).unwrap(),
            "old"
        );
        std::fs::write(pending.join(FILES[1]), "corrupt").unwrap();
        assert!(replace_pair(&pending, &install).is_err());
        assert_eq!(
            std::fs::read_to_string(install.join(FILES[1])).unwrap(),
            "new peripheral-taskbar-hook.dll"
        );
    }
    #[test]
    fn locked_dll_rolls_back_executable_and_keeps_package() {
        use std::os::windows::fs::OpenOptionsExt;
        let install = fixture();
        let pending = install.join("updates/pending");
        std::fs::create_dir_all(&pending).unwrap();
        package(&pending);
        for name in FILES {
            std::fs::write(install.join(name), "old").unwrap();
        }
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(install.join(FILES[1]))
            .unwrap();
        assert!(replace_pair(&pending, &install).is_err());
        for name in FILES {
            assert_eq!(std::fs::read_to_string(install.join(name)).unwrap(), "old");
        }
        assert!(verify_package(&pending).is_ok());
        drop(lock);
        replace_pair(&pending, &install).unwrap();
    }
}
