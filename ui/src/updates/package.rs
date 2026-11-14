use super::*;

pub fn verify_package(folder: &Path) -> Result<Value> {
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(folder.join("release-manifest.json")).context("release manifest missing")?,
    )?;
    anyhow::ensure!(
        manifest["format"] == 1 && manifest["architecture"] == "x64",
        "unsupported package format or architecture"
    );
    verify_files(folder, &manifest)?;
    Ok(manifest)
}

/// Compare actual binaries against a specific target, never just the local manifest.
pub(super) fn verify_files(folder: &Path, manifest: &Value) -> Result<()> {
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
    Ok(())
}
pub fn stage(source: &Path, installation: &Path) -> Result<Value> {
    let _guard = operation::Guard::acquire(installation)?;
    operation::progress(
        installation,
        "staging",
        "Verifying the selected update package",
    )?;
    let outcome = stage_locked(source, installation);
    if let Err(error) = &outcome {
        operation::progress(
            installation,
            "error",
            &format!("Prepare update failed: {error:#}"),
        )?;
    }
    outcome
}
fn stage_locked(source: &Path, installation: &Path) -> Result<Value> {
    let manifest = verify_package(source)?;
    anyhow::ensure!(
        verify_files(installation, &manifest).is_err(),
        "Selected EXE/DLL are already installed in {}; no update is needed",
        installation.display()
    );
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
    operation::progress(installation, "ready", "Update prepared; not installed yet")?;
    Ok(value)
}
pub(super) fn replace_pair(pending: &Path, installation: &Path) -> Result<()> {
    verify_package(pending)?;
    let manifest_path = installation.join("release-manifest.json");
    let previous_manifest = if manifest_path.exists() {
        Some(std::fs::read(&manifest_path)?)
    } else {
        None
    };
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
        verify_package(installation).context("installed binary pair failed verification")?;
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
        let restored = match previous_manifest {
            Some(bytes) => std::fs::write(&manifest_path, bytes),
            None if manifest_path.is_file() => std::fs::remove_file(&manifest_path),
            None => Ok(()),
        };
        if let Err(error) = restored {
            rollback_errors.push(format!("restore manifest: {error}"));
        }
        anyhow::bail!("update failed: {error:#}; rollback errors: {rollback_errors:?}");
    }
    Ok(())
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
    #[test]
    fn manifest_write_failure_restores_both_binaries() {
        use std::os::windows::fs::OpenOptionsExt;
        let install = fixture();
        let pending = install.join("updates/pending");
        std::fs::create_dir_all(&pending).unwrap();
        package(&pending);
        package(&install);
        let old_manifest = std::fs::read(install.join("release-manifest.json")).unwrap();
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(install.join("release-manifest.json"))
            .unwrap();
        std::fs::write(pending.join(FILES[0]), "newer exe").unwrap();
        let mut manifest: Value =
            serde_json::from_slice(&std::fs::read(pending.join("release-manifest.json")).unwrap())
                .unwrap();
        manifest["files"][0]["sha256"] =
            json!(crate::distribution::hash(&pending.join(FILES[0])).unwrap());
        atomic_json(&pending.join("release-manifest.json"), &manifest).unwrap();
        assert!(replace_pair(&pending, &install).is_err());
        drop(lock);
        assert_eq!(
            std::fs::read(install.join("release-manifest.json")).unwrap(),
            old_manifest
        );
        verify_package(&install).unwrap();
        verify_package(&pending).unwrap();
        std::fs::remove_dir_all(install).unwrap();
    }
}
