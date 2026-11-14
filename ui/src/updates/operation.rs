use super::*;

/// One staging/restart/apply operation per physical installation, across processes.
pub(super) struct Guard(HANDLE);
impl Guard {
    pub fn acquire(root: &Path) -> Result<Self> {
        use sha2::{Digest, Sha256};
        let identity = std::fs::canonicalize(root)?
            .to_string_lossy()
            .to_lowercase();
        let name = HSTRING::from(format!(
            "Local\\PeripheralUpdate_{:x}",
            Sha256::digest(identity.as_bytes())
        ));
        unsafe {
            let handle = CreateMutexW(None, false, &name)?;
            let result = WaitForSingleObject(handle, 0);
            if result != WAIT_OBJECT_0 && result != WAIT_ABANDONED {
                let _ = CloseHandle(handle);
                anyhow::bail!(
                    "An update or Explorer restart is already running for this installation"
                );
            }
            Ok(Self(handle))
        }
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}

pub(super) fn progress(root: &Path, state: &str, message: &str) -> Result<()> {
    std::fs::create_dir_all(root.join("updates"))?;
    let manifest = |folder: &Path| -> Value {
        std::fs::read(folder.join("release-manifest.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .map(|v| v["files"].clone())
            .unwrap_or(Value::Null)
    };
    let value = json!({
        "state": state, "message": message, "worker_pid": std::process::id(),
        "unix_ms": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis(),
        "installation": std::fs::canonicalize(root)?,
        "worker_executable": std::env::current_exe()?,
        "target_files": manifest(&root.join("updates/pending")),
        "installed_manifest_files": manifest(root)
    });
    atomic_json(&root.join("updates/result.json"), &value)?;
    // Preserve previous phases when the final status replaces result.json.
    // Logging failure must not turn a completed installation into a failure.
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("updates/history.jsonl"))
    {
        let _ = writeln!(file, "{value}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn another_worker_cannot_restart_or_replace_concurrently() {
        let root =
            std::env::temp_dir().join(format!("peripheral-operation-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let guard = Guard::acquire(&root).unwrap();
        let other = root.clone();
        assert!(
            std::thread::spawn(move || Guard::acquire(&other).is_err())
                .join()
                .unwrap()
        );
        drop(guard);
        let other = root.clone();
        assert!(
            std::thread::spawn(move || Guard::acquire(&other).is_ok())
                .join()
                .unwrap()
        );
        std::fs::remove_dir(root).unwrap();
    }
}
