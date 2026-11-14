//! Execute the updater from the currently running version, not the target package.
use super::*;

pub(super) fn prepare(installation: &Path, ticket: &str) -> Result<PathBuf> {
    let ticket = uuid::Uuid::parse_str(ticket)?;
    let folder = installation.join("updates/runner").join(ticket.to_string());
    std::fs::create_dir_all(&folder)?;
    let source = std::env::current_exe()?;
    let executable = folder.join(FILES[0]);
    std::fs::copy(&source, &executable).context("prepare independent updater")?;
    anyhow::ensure!(
        crate::distribution::hash(&source)? == crate::distribution::hash(&executable)?,
        "updater copy verification failed"
    );
    Ok(executable)
}

pub(super) fn installation(executable_folder: &Path) -> Result<PathBuf> {
    let parent = executable_folder
        .parent()
        .context("updater parent directory")?;
    if executable_folder
        .file_name()
        .is_some_and(|n| n == "pending")
        && parent.file_name().is_some_and(|n| n == "updates")
    {
        return Ok(parent
            .parent()
            .context("installation directory")?
            .to_owned());
    }
    anyhow::ensure!(
        parent.file_name().is_some_and(|n| n == "runner"),
        "invalid updater location"
    );
    uuid::Uuid::parse_str(
        executable_folder
            .file_name()
            .and_then(|s| s.to_str())
            .context("runner identity")?,
    )?;
    let updates = parent.parent().context("updates directory")?;
    anyhow::ensure!(
        updates.file_name().is_some_and(|n| n == "updates"),
        "invalid runner parent"
    );
    Ok(updates
        .parent()
        .context("installation directory")?
        .to_owned())
}

pub(super) struct ParentProcess(Option<HANDLE>);
impl ParentProcess {
    pub fn capture(pid: u32) -> Result<Self> {
        unsafe {
            match OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
                Ok(handle) => Ok(Self(Some(handle))),
                Err(e)
                    if e.code()
                        == windows::core::HRESULT::from_win32(ERROR_INVALID_PARAMETER.0) =>
                {
                    Ok(Self(None))
                }
                Err(e) => Err(e.into()),
            }
        }
    }
    pub fn wait(&self) -> Result<()> {
        if let Some(handle) = self.0 {
            anyhow::ensure!(
                unsafe { WaitForSingleObject(handle, 30000) } == WAIT_OBJECT_0,
                "Old UI did not exit within 30 seconds; Explorer was not touched and the update was retained"
            );
        }
        Ok(())
    }
    pub fn exited(&self) -> bool {
        self.0
            .is_none_or(|h| unsafe { WaitForSingleObject(h, 0) == WAIT_OBJECT_0 })
    }
}
impl Drop for ParentProcess {
    fn drop(&mut self) {
        if let Some(h) = self.0 {
            unsafe {
                let _ = CloseHandle(h);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn runner_and_legacy_pending_resolve_the_same_installation() {
        let root = std::env::temp_dir().join("peripheral-location-test");
        assert_eq!(installation(&root.join("updates/pending")).unwrap(), root);
        assert_eq!(
            installation(
                &root
                    .join("updates/runner")
                    .join(uuid::Uuid::new_v4().to_string())
            )
            .unwrap(),
            root
        );
        assert!(installation(&root.join("runner/random")).is_err());
        assert!(installation(&root.join("updates/runner/not-a-ticket")).is_err());
    }
}
