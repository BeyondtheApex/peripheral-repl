use super::*;

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

/// The display preference is not evidence that Explorer loaded this DLL.
/// Unmounted installations must be updatable without any shell restart.
pub(super) fn installation_loaded(installation: &Path) -> Result<bool> {
    let Some(pid) = explorer_pid() else {
        return Ok(false);
    };
    let modules = loaded_modules(pid)?;
    let expected = installation.join(FILES[1]).canonicalize()?;
    Ok(modules
        .iter()
        .any(|path| path.canonicalize().ok().as_ref() == Some(&expected)))
}

/// Query the opened process, not a name-based enumeration that could select
/// unrelated Explorer folder processes or a reused PID.
pub(crate) fn process_image(pid: u32) -> Result<PathBuf> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)?;
        let result = image(process);
        let _ = CloseHandle(process);
        result
    }
}
fn image(handle: HANDLE) -> Result<PathBuf> {
    let mut path = vec![0u16; 32768];
    let mut length = path.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(path.as_mut_ptr()),
            &mut length,
        )?;
    }
    Ok(PathBuf::from(String::from_utf16_lossy(
        &path[..length as usize],
    )))
}
pub(super) fn shell_exe() -> Result<PathBuf> {
    Ok(
        PathBuf::from(std::env::var_os("SystemRoot").context("Windows directory unavailable")?)
            .join("explorer.exe"),
    )
}
fn wait_then_stop(handle: HANDLE, grace_ms: u32, allow_stop: bool) -> Result<()> {
    unsafe {
        match WaitForSingleObject(handle, grace_ms) {
            WAIT_OBJECT_0 => return Ok(()),
            WAIT_TIMEOUT => (),
            _ => anyhow::bail!("Cannot wait for the selected Explorer process"),
        }
        // Only the process handle captured from the primary taskbar. The GUI
        // confirmation explicitly describes this fallback; never taskkill /IM.
        anyhow::ensure!(
            allow_stop,
            "Explorer did not exit gracefully. The older UI did not request termination; use the new UI confirmation or restart Explorer manually"
        );
        TerminateProcess(handle, 0)
            .context("The selected Explorer process did not exit and could not be stopped")?;
        anyhow::ensure!(
            WaitForSingleObject(handle, 5000) == WAIT_OBJECT_0,
            "Explorer did not exit within the restart deadline"
        );
    }
    Ok(())
}

pub(super) struct ShellProcess {
    handle: HANDLE,
    pid: u32,
    executable: PathBuf,
    recover: bool,
}
impl ShellProcess {
    pub fn capture() -> Result<Self> {
        let pid = explorer_pid().context("Explorer taskbar is not available")?;
        let executable = shell_exe()?;
        let handle = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | PROCESS_TERMINATE,
                false,
                pid,
            )?
        };
        let valid = image(handle).and_then(|actual| {
            Ok(std::fs::canonicalize(actual)? == std::fs::canonicalize(&executable)?)
        });
        if !matches!(valid, Ok(true)) {
            unsafe {
                let _ = CloseHandle(handle);
            }
            anyhow::bail!("The taskbar owner is not the system Explorer executable");
        }
        Ok(Self {
            handle,
            pid,
            executable,
            recover: false,
        })
    }
    pub fn stop(mut self, allow_stop: bool) -> Result<Self> {
        self.recover = true;
        unsafe {
            if let Ok(bar) = FindWindowW(w!("Shell_TrayWnd"), None) {
                let mut owner = 0;
                GetWindowThreadProcessId(bar, Some(&mut owner));
                if owner == self.pid {
                    // Explorer's Exit Explorer request is opportunistic. If this
                    // private message is unsupported, the bounded handle fallback applies.
                    let _ = PostMessageW(Some(bar), WM_USER + 436, WPARAM(0), LPARAM(0));
                }
            }
        }
        wait_then_stop(self.handle, 3000, allow_stop)?;
        Ok(self)
    }
    pub fn resume(&mut self) -> Result<()> {
        if explorer_pid().is_none() {
            Command::new(&self.executable)
                .current_dir(self.executable.parent().unwrap())
                .creation_flags(0x08000000)
                .spawn()?;
        }
        let deadline = Instant::now() + Duration::from_secs(12);
        while Instant::now() < deadline {
            if explorer_pid().is_some_and(|pid| pid != self.pid) {
                self.recover = false;
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        anyhow::bail!(
            "Explorer did not recreate its taskbar within 12 seconds; update status is retained in updates/result.json"
        )
    }
}
impl Drop for ShellProcess {
    fn drop(&mut self) {
        if self.recover && explorer_pid().is_none() {
            let _ = Command::new(&self.executable)
                .creation_flags(0x08000000)
                .spawn();
        }
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unresponsive_process_has_a_bounded_stop_without_touching_explorer() {
        let mut child = Command::new("powershell.exe")
            .args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"])
            .creation_flags(0x08000000)
            .spawn()
            .unwrap();
        let handle = unsafe {
            OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, false, child.id()).unwrap()
        };
        let start = Instant::now();
        let result = wait_then_stop(handle, 50, true);
        unsafe {
            let _ = CloseHandle(handle);
        }
        if result.is_err() {
            let _ = child.kill();
        }
        let _ = child.wait();
        result.unwrap();
        assert!(start.elapsed() < Duration::from_secs(7));
    }
    #[test]
    fn legacy_request_cannot_terminate_an_unresponsive_process() {
        let mut child = Command::new("powershell.exe")
            .args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"])
            .creation_flags(0x08000000)
            .spawn()
            .unwrap();
        let handle = unsafe {
            OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, false, child.id()).unwrap()
        };
        let rejected = wait_then_stop(handle, 10, false).is_err();
        let still_running = unsafe { WaitForSingleObject(handle, 0) == WAIT_TIMEOUT };
        unsafe {
            let _ = CloseHandle(handle);
        }
        let _ = child.kill();
        let _ = child.wait();
        assert!(rejected && still_running);
    }
}

pub(super) fn ensure_shell_running() -> Result<()> {
    if explorer_pid().is_none() {
        Command::new(shell_exe()?)
            .creation_flags(0x08000000)
            .spawn()?;
    }
    Ok(())
}
