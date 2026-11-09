//! Script catalog and bounded imports. Runtime execution stays in runtime.rs.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
pub fn scripts(root: &Path) -> Result<Value> {
    let mut pending = vec![(root.to_owned(), 0)];
    let mut files = Vec::new();
    let mut visited = 0;
    while let Some((dir, depth)) = pending.pop() {
        if depth > 5 {
            continue;
        }
        for entry in std::fs::read_dir(dir)? {
            visited += 1;
            ensure!(
                visited <= 20000,
                "script catalog too large; choose a narrower workspace"
            );
            let entry = entry?;
            let ty = entry.file_type()?;
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if ty.is_symlink() {
                continue;
            }
            if ty.is_dir() {
                if [
                    "target",
                    "tests",
                    "ui",
                    ".git",
                    ".research",
                    "node_modules",
                    "crates",
                ]
                .contains(&name.as_ref())
                {
                    continue;
                }
                pending.push((path, depth + 1));
            } else if ty.is_file() && path.extension().is_some_and(|e| e == "lua") {
                ensure!(files.len() < 500, "script catalog exceeds 500 entries");
                files.push(json!({"path":path.strip_prefix(root)?.to_string_lossy().replace('\\',"/"),"name":name,"bytes":entry.metadata()?.len()}));
            }
        }
    }
    files.sort_by_key(|v| v["path"].as_str().unwrap_or("").to_owned());
    Ok(json!(files))
}
pub fn read(root: &Path, path: &str) -> Result<Value> {
    let full = inside(root, path)?;
    let metadata = std::fs::metadata(&full)?;
    ensure!(
        metadata.len() <= crate::runtime::MAX_SOURCE as u64,
        "script too large"
    );
    Ok(json!({"path":path,"source":std::fs::read_to_string(full)?}))
}
pub fn import(root: &Path, name: &str, source: &str) -> Result<Value> {
    let stem = name.strip_suffix(".lua").unwrap_or(name);
    ensure!(
        peripheral_ui_protocol::identifier(stem),
        "script name must use letters, digits, _ or -"
    );
    ensure!(
        source.len() <= crate::runtime::MAX_SOURCE,
        "script too large"
    );
    let dir = root.join("scripts/user");
    std::fs::create_dir_all(&dir)?;
    ensure!(
        dir.canonicalize()?.starts_with(root),
        "script directory escapes workspace"
    );
    let path = dir.join(format!("{stem}.lua"));
    ensure!(
        !path.exists(),
        "script already exists; import under another name"
    );
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)?;
    file.write_all(source.as_bytes())?;
    Ok(json!({"path":format!("scripts/user/{stem}.lua")}))
}
fn inside(root: &Path, path: &str) -> Result<PathBuf> {
    let full = root.join(path).canonicalize().context("script path")?;
    ensure!(
        full.starts_with(root) && full.extension().is_some_and(|e| e == "lua"),
        "script path must be a Lua file inside the workspace"
    );
    Ok(full)
}
