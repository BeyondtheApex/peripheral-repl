//! Graceful backend process control; never kill an unresponsive daemon.
use super::*;
pub(super) fn start(ep: &Path, root: &Path) -> Result<()> {
    if request(ep, json!({"op":"ping"})).is_ok() {
        return Ok(());
    }
    if ep.exists() {
        let old: peripheral_repl::protocol::Endpoint = serde_json::from_slice(&std::fs::read(ep)?)?;
        anyhow::ensure!(
            !crate::native::process_alive(old.pid),
            "后台进程仍存在但无法连接，未启动重复进程"
        );
        std::fs::remove_file(ep)?;
    }
    std::process::Command::new(std::env::current_exe()?)
        .arg("--daemon")
        .arg(root)
        .creation_flags(0x08000000)
        .spawn()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while request(ep, json!({"op":"ping"})).is_err() {
        anyhow::ensure!(std::time::Instant::now() < deadline, "后台启动超时");
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}
pub(super) fn stop(ep: &Path) -> Result<()> {
    if !ep.exists() {
        return Ok(());
    }
    let old: peripheral_repl::protocol::Endpoint = serde_json::from_slice(&std::fs::read(ep)?)?;
    if !crate::native::process_alive(old.pid) {
        return Ok(());
    }
    request(ep, json!({"op":"shutdown"}))?;
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while crate::native::process_alive(old.pid) || ep.exists() {
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "后台尚未退出，未强制终止进程"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}
