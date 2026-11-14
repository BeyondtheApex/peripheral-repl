//! Confirm that the relaunched GUI responds, rather than treating spawn as success.
use super::*;
use std::{
    fs::OpenOptions,
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpStream},
    process::Stdio,
};

pub(super) fn start(installation: &Path, resume: crate::backend::Resume) -> Result<()> {
    let log_path = installation.join("updates/ui-startup.log");
    let endpoint = crate::backend::data_dir().join("session.json");
    for attempt in 1..=2 {
        let mut log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;
        writeln!(log, "\nUpdate GUI launch attempt {attempt}")?;
        let mut command = Command::new(installation.join(FILES[0]));
        if let Some(argument) = resume.ui_argument() {
            command.arg(argument);
        }
        let mut child = command
            .current_dir(installation)
            .creation_flags(0x08000000)
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log.try_clone()?))
            .spawn()?;
        writeln!(log, "GUI process {}", child.id())?;
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = child.try_wait()? {
                writeln!(log, "GUI exited before ready: {status}")?;
                anyhow::ensure!(
                    attempt < 2,
                    "UI exited before becoming ready ({status}); see {}",
                    log_path.display()
                );
                // Explorer can still be initializing its desktop after recreating
                // the taskbar. Retry only an exited child, never duplicate a live GUI.
                std::thread::sleep(Duration::from_millis(750));
                break;
            }
            if ready(&endpoint, child.id()).unwrap_or(false) {
                writeln!(log, "GUI ready: {}", child.id())?;
                return Ok(());
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "UI process {} did not respond within 15 seconds; it was left running. See {}",
                child.id(),
                log_path.display()
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    unreachable!("the second early exit returns an error")
}

fn ready(endpoint: &Path, pid: u32) -> Result<bool> {
    let file = std::fs::File::open(endpoint)?;
    let mut bytes = Vec::new();
    file.take(4097).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= 4096, "oversized UI endpoint");
    let value: Value = serde_json::from_slice(&bytes)?;
    if value["pid"] != pid {
        return Ok(false);
    }
    let address: SocketAddr = value["address"].as_str().context("UI address")?.parse()?;
    anyhow::ensure!(address.ip().is_loopback(), "UI endpoint must be local");
    let token = value["token"].as_str().context("UI token")?;
    let mut socket = TcpStream::connect_timeout(&address, Duration::from_millis(250))?;
    socket.set_write_timeout(Some(Duration::from_millis(250)))?;
    socket.set_read_timeout(Some(Duration::from_millis(500)))?;
    writeln!(socket, "{}", json!({"op":"ping","token":token}))?;
    let mut line = String::new();
    BufReader::new(socket).take(8193).read_line(&mut line)?;
    anyhow::ensure!(
        line.len() <= 8192 && line.ends_with('\n'),
        "invalid UI reply"
    );
    let reply: Value = serde_json::from_str(&line)?;
    // A rolled-back older UI has no PID in its ping response. Its endpoint
    // must still have been written by this exact child process.
    let identity_matches = reply["result"]["pid"] == pid
        || (reply["result"]["pid"].is_null()
            && reply["result"]["transport"] == "authenticated loopback JSON");
    Ok(reply["ok"] == true && identity_matches)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn readiness_requires_a_reply_from_the_new_ui_process() {
        let root =
            std::env::temp_dir().join(format!("peripheral-ui-ready-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("session.json");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let pid = std::process::id();
        std::fs::write(
            &path,
            json!({"address":listener.local_addr().unwrap(),"pid":pid,"token":"readiness-test"})
                .to_string(),
        )
        .unwrap();
        assert!(!ready(&path, pid + 1).unwrap());
        let server = std::thread::spawn(move || {
            for result in [
                json!({"pid":pid}),
                json!({"transport":"authenticated loopback JSON"}),
                json!({"pid":pid+1}),
            ] {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut line = String::new();
                BufReader::new(&socket).read_line(&mut line).unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(request, json!({"op":"ping","token":"readiness-test"}));
                writeln!(socket, "{}", json!({"ok":true,"result":result})).unwrap();
            }
        });
        assert!(ready(&path, pid).unwrap());
        assert!(ready(&path, pid).unwrap());
        assert!(!ready(&path, pid).unwrap());
        server.join().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn readiness_rejects_nonlocal_or_oversized_endpoints() {
        let root =
            std::env::temp_dir().join(format!("peripheral-ui-ready-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("session.json");
        std::fs::write(
            &path,
            json!({"address":"192.0.2.1:12","pid":1,"token":"unused"}).to_string(),
        )
        .unwrap();
        assert!(ready(&path, 1).is_err());
        std::fs::write(&path, vec![b' '; 4097]).unwrap();
        assert!(ready(&path, 1).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
