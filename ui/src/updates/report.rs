//! Read-only binary inspection; no shell restart, backend connection or DLL loading.
use super::*;

pub fn write(root: &Path) -> Result<PathBuf> {
    let root = std::fs::canonicalize(root).context("installation to inspect")?;
    let inventory = |folder: &Path| -> Value {
        json!(
            FILES
                .iter()
                .map(|name| {
                    let path = folder.join(name);
                    match crate::distribution::hash(&path) {
                        Ok(hash) => json!({"path":path,"sha256":hash}),
                        Err(error) => json!({"path":path,"error":format!("{error:#}")}),
                    }
                })
                .collect::<Vec<_>>()
        )
    };
    let pending = root.join("updates/pending");
    let target = verify_package(&pending);
    let matches = target
        .as_ref()
        .ok()
        .map(|manifest| package::verify_files(&root, manifest).is_ok());
    let value = json!({
        "installation":root,
        "report_executable":std::env::current_exe()?,
        "installed":inventory(&root),
        "pending":inventory(&pending),
        "previous":inventory(&root.join("updates/previous")),
        "pending_marker":std::fs::read(pending.join("update.json")).ok().and_then(|b|serde_json::from_slice::<Value>(&b).ok()),
        "pending_verification_error":target.err().map(|e|format!("{e:#}")),
        "installed_matches_pending":matches,
        "last_result":result(&root),
        "running_ui_processes":running_ui_processes()?,
        "explorer_pid":explorer_pid(),
        "loaded_dlls":explorer_pid().and_then(|pid|loaded_modules(pid).ok()),
    });
    std::fs::create_dir_all(root.join("updates"))?;
    let path = root.join("updates/diagnostics.json");
    atomic_json(&path, &value)?;
    Ok(path)
}

fn running_ui_processes() -> Result<Vec<Value>> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?;
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut processes = Vec::new();
        let mut next = Process32FirstW(snapshot, &mut entry);
        while next.is_ok() {
            let end = entry
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len());
            if String::from_utf16_lossy(&entry.szExeFile[..end]).eq_ignore_ascii_case(FILES[0]) {
                let path = process_image(entry.th32ProcessID);
                processes.push(json!({"pid":entry.th32ProcessID,"path":path.as_ref().ok(),"error":path.as_ref().err().map(|e|format!("{e:#}"))}));
            }
            next = Process32NextW(snapshot, &mut entry);
        }
        let _ = CloseHandle(snapshot);
        Ok(processes)
    }
}
