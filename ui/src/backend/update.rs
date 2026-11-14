//! Graceful update shutdown of this installation only. No device operations.
use super::*;
pub(crate) fn stop_for_update(endpoint: &Path, installation: &Path) -> Result<Resume> {
    let bytes = match std::fs::read(endpoint) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Resume::Stopped),
        Err(error) => return Err(error).context("Read backend endpoint before update"),
    };
    let old: peripheral_repl::protocol::Endpoint = serde_json::from_slice(&bytes)?;
    let Some(process) = BackendProcess::open(old.pid)? else {
        return Ok(Resume::Stopped);
    };
    let actual = crate::updates::process_image(old.pid);
    if process.exited() {
        return Ok(Resume::Stopped);
    }
    if actual?.canonicalize()? != installation.join("peripheral-ui.exe").canonicalize()? {
        // A global endpoint may belong to a different installation or a stale
        // PID. It cannot lock our EXE. Never stop or restart that process.
        return Ok(Resume::Stopped);
    }
    anyhow::ensure!(
        old.address.ip().is_loopback(),
        "Backend endpoint must be local"
    );
    let resume = match request(endpoint, json!({"op":"ping"})) {
        Ok(ping) if ping["host"]["elevated"] == true => Resume::Elevated,
        Ok(_) => Resume::Normal,
        Err(_) if process.exited() => return Ok(Resume::Stopped),
        Err(error) => return Err(error).context("Read backend privilege mode before update"),
    };
    let shutdown = (|| -> Result<()> {
        let mut stream = TcpStream::connect_timeout(&old.address, Duration::from_secs(2))?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        serde_json::to_writer(
            &mut stream,
            &json!({"token":old.token,"command":{"op":"shutdown"}}),
        )?;
        stream.write_all(b"\n")?;
        let mut reply = String::new();
        use std::io::Read;
        BufReader::new(stream).take(65537).read_line(&mut reply)?;
        let reply: Value = serde_json::from_str(&reply)?;
        anyhow::ensure!(
            reply["ok"] == true,
            "Backend refused update shutdown: {}",
            reply["error"]
        );
        Ok(())
    })();
    // Normal shutdown can race socket connection/reply. The held process handle
    // proves this exact process exited; a recycled PID must not block the update.
    if process.exited() {
        return Ok(resume);
    }
    shutdown?;
    let deadline = Instant::now() + Duration::from_secs(8);
    while !process.exited() {
        anyhow::ensure!(
            Instant::now() < deadline,
            "Backend did not exit within 8 seconds; update cancelled without forcing it to exit"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(resume)
}

use windows::Win32::{Foundation::*, System::Threading::*};
struct BackendProcess(HANDLE);
impl BackendProcess {
    fn open(pid: u32) -> Result<Option<Self>> {
        unsafe {
            match OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
                Ok(handle) => {
                    let process = Self(handle);
                    Ok((!process.exited()).then_some(process))
                }
                Err(e)
                    if e.code()
                        == windows::core::HRESULT::from_win32(ERROR_INVALID_PARAMETER.0) =>
                {
                    Ok(None)
                }
                Err(e) => Err(e.into()),
            }
        }
    }
    fn exited(&self) -> bool {
        unsafe { WaitForSingleObject(self.0, 0) == WAIT_OBJECT_0 }
    }
}
impl Drop for BackendProcess {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absent_dead_and_foreign_backends_do_not_block_updates() {
        let root = std::env::temp_dir().join(format!(
            "peripheral-backend-update-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let endpoint = root.join("endpoint.json");
        assert_eq!(stop_for_update(&endpoint, &root).unwrap(), Resume::Stopped);
        // A syntactically valid but stale endpoint; no network request is needed.
        let value = json!({"pid":u32::MAX,"address":"127.0.0.1:1","token":"unused","root":root});
        std::fs::write(&endpoint, serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(stop_for_update(&endpoint, &root).unwrap(), Resume::Stopped);
        std::fs::write(root.join("peripheral-ui.exe"), "different installation").unwrap();
        let mut value = value;
        value["pid"] = json!(std::process::id());
        std::fs::write(&endpoint, serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(stop_for_update(&endpoint, &root).unwrap(), Resume::Stopped);
        // Corrupt metadata must be surfaced, not silently treated as stopped.
        std::fs::write(&endpoint, b"broken").unwrap();
        assert!(stop_for_update(&endpoint, &root).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
