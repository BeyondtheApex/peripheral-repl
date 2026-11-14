//! Reconcile interrupted/legacy update markers against the actual installed pair.
use super::*;

pub fn reconcile(root: &Path) -> Result<bool> {
    if pending(root).is_none() {
        return Ok(false);
    }
    let _guard = operation::Guard::acquire(root)?;
    if pending(root).is_none() {
        return Ok(false);
    }
    let target = verify_package(&root.join("updates/pending"))?;
    // A previous `applied` message is not evidence. Both files must match.
    if package::verify_files(root, &target).is_err() {
        return Ok(false);
    }
    atomic_json(&root.join("release-manifest.json"), &target)?;
    atomic_json(
        &root.join("updates/pending/update.json"),
        &json!({"state":"applied","manifest":target}),
    )?;
    operation::progress(
        root,
        "applied",
        "Installed EXE/DLL hashes match the pending package; recovered stale update marker",
    )?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (PathBuf, Value) {
        let root =
            std::env::temp_dir().join(format!("peripheral-recovery-{}", uuid::Uuid::new_v4()));
        let pending = root.join("updates/pending");
        std::fs::create_dir_all(&pending).unwrap();
        let files: Vec<_> = FILES.iter().map(|name| {
            std::fs::write(pending.join(name), format!("target {name}")).unwrap();
            std::fs::write(root.join(name), "old").unwrap();
            json!({"name":name,"sha256":crate::distribution::hash(&pending.join(name)).unwrap()})
        }).collect();
        let manifest = json!({"format":1,"architecture":"x64","files":files});
        atomic_json(&pending.join("release-manifest.json"), &manifest).unwrap();
        atomic_json(
            &pending.join("update.json"),
            &json!({"state":"ready","manifest":manifest}),
        )
        .unwrap();
        (root, manifest)
    }
    #[test]
    fn stale_success_or_copied_manifest_cannot_clear_a_real_update() {
        let (root, manifest) = fixture();
        operation::progress(&root, "applied", "old success").unwrap();
        atomic_json(&root.join("release-manifest.json"), &manifest).unwrap();
        assert!(!reconcile(&root).unwrap());
        assert!(pending(&root).is_some());
        std::fs::copy(
            root.join("updates/pending").join(FILES[0]),
            root.join(FILES[0]),
        )
        .unwrap();
        assert!(!reconcile(&root).unwrap());
        assert!(pending(&root).is_some());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn identical_pair_recovers_once_and_preserves_previous_backup() {
        let (root, _) = fixture();
        std::fs::create_dir_all(root.join("updates/previous")).unwrap();
        for name in FILES {
            std::fs::write(root.join("updates/previous").join(name), "backup").unwrap();
            std::fs::copy(root.join("updates/pending").join(name), root.join(name)).unwrap();
        }
        assert!(reconcile(&root).unwrap());
        assert!(pending(&root).is_none());
        assert!(!reconcile(&root).unwrap());
        for name in FILES {
            assert_eq!(
                std::fs::read_to_string(root.join("updates/previous").join(name)).unwrap(),
                "backup"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn staging_same_bytes_is_rejected_and_replaces_historical_success() {
        let (root, _) = fixture();
        for name in FILES {
            std::fs::copy(root.join("updates/pending").join(name), root.join(name)).unwrap();
        }
        operation::progress(&root, "applied", "old success").unwrap();
        assert!(
            stage(&root.join("updates/pending"), &root)
                .unwrap_err()
                .to_string()
                .contains("already installed")
        );
        assert_eq!(result(&root).unwrap()["state"], "error");
        std::fs::remove_dir_all(root).unwrap();
    }
}
