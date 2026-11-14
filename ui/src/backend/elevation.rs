//! UAC restarts the existing daemon mode; the GUI and MCP bridge stay unelevated.
//! Approval happens before shutdown. A cancelled prompt leaves the old daemon alive.
use super::*;
use windows::{
    Win32::{
        Foundation::CloseHandle,
        System::Threading::GetExitCodeProcess,
        UI::{
            Shell::{
                SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
                ShellExecuteExW,
            },
            WindowsAndMessaging::SW_HIDE,
        },
    },
    core::{HSTRING, PCWSTR, w},
};

pub(super) fn start(ep: &Path, root: &Path, restart: bool) -> Result<()> {
    let prior = request(ep, json!({"op":"ping"})).ok();
    if prior
        .as_ref()
        .is_some_and(|p| p["host"]["elevated"] == true)
        && !restart
    {
        return Ok(());
    }
    let active = prior
        .as_ref()
        .and_then(|p| p["root"].as_str())
        .map(PathBuf::from)
        .unwrap_or_else(|| root.to_owned())
        .canonicalize()?;
    let expected = prior
        .as_ref()
        .and_then(|p| p["runtime_id"].as_str())
        .unwrap_or("absent");
    // Resolve application/container redirects before crossing the UAC boundary.
    let exe = shell_path(&std::env::current_exe()?)?;
    peripheral_repl::protocol::prepare_endpoint_directory(ep)?;
    let endpoint = shell_path(ep.parent().context("endpoint directory")?)?
        .join(ep.file_name().context("endpoint filename")?);
    let sid = peripheral_repl::host::user_sid()?;
    let ticket = uuid::Uuid::new_v4().to_string();
    let report = endpoint.with_file_name(format!("elevation-{ticket}.json"));
    let arguments = [
        "--daemon-elevated".to_owned(),
        active.to_string_lossy().into_owned(),
        endpoint.to_string_lossy().into_owned(),
        expected.to_owned(),
        sid,
        ticket,
    ];
    let process = launch(&exe, &arguments)?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let identity: Value = std::fs::read(&report)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or(Value::Null);
        if let Some(error) = identity["error"].as_str() {
            anyhow::bail!("{error}");
        }
        if let Ok(ping) = request(ep, json!({"op":"ping"}))
            && ping["host"]["elevated"] == true
            && ping["host"]["pid"] == identity["pid"]
            && ping["runtime_id"] == identity["runtime_id"]
            && ping["runtime_id"] != expected
            && Path::new(ping["root"].as_str().unwrap_or("")) == active
        {
            let _ = std::fs::remove_file(&report);
            return Ok(());
        }
        let mut code = 259;
        unsafe { GetExitCodeProcess(process.0, &mut code) }?;
        anyhow::ensure!(
            code == 259,
            "Elevated backend exited with code {code}. Check {}",
            report.display()
        );
        anyhow::ensure!(
            Instant::now() < deadline,
            "Elevated backend did not become ready within 30 seconds; no process was killed"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

struct Process(windows::Win32::Foundation::HANDLE);
impl Drop for Process {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
fn launch(exe: &Path, arguments: &[String]) -> Result<Process> {
    let parameters = HSTRING::from(
        arguments
            .iter()
            .map(|s| quote(s))
            .collect::<Vec<_>>()
            .join(" "),
    );
    let file = HSTRING::from(exe.as_os_str());
    let directory = HSTRING::from(exe.parent().context("executable directory")?.as_os_str());
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(parameters.as_ptr()),
        lpDirectory: PCWSTR(directory.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }
        .context("Could not start elevated backend; existing backend was kept running")?;
    Ok(Process(info.hProcess))
}
fn shell_path(path: &Path) -> Result<PathBuf> {
    let path = path.canonicalize()?;
    let text = path.to_str().context("path must be valid Unicode")?;
    Ok(if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path
    })
}

/// Executed after UAC. Never opens windows, changes device grants, or starts a second GUI.
pub(crate) fn run(args: &[String]) -> Result<()> {
    anyhow::ensure!(
        args.len() == 5,
        "usage: --daemon-elevated <root> <endpoint> <expected-runtime|absent> <user-sid> <ticket>"
    );
    let root = Path::new(&args[0]);
    let ep = Path::new(&args[1]);
    let ticket = uuid::Uuid::parse_str(&args[4])?;
    let report = ep.with_file_name(format!("elevation-{ticket}.json"));
    anyhow::ensure!(
        peripheral_repl::host::elevation()?,
        "backend process is not elevated"
    );
    anyhow::ensure!(
        peripheral_repl::host::user_sid()? == args[3],
        "Elevate the same Windows user; switching to a different administrator account is not supported"
    );
    let result = (|| {
        // Validate policy and workspace before touching the running daemon.
        let (runtime, root) = super::prepare_daemon(root)?;
        peripheral_repl::protocol::prepare_endpoint_directory(ep)?;
        let identity = runtime.dispatch(json!({"op":"ping"}))?;
        std::fs::write(
            &report,
            json!({"pid":std::process::id(),"runtime_id":identity["runtime_id"]}).to_string(),
        )?;
        if args[2] == "absent" {
            anyhow::ensure!(
                request(ep, json!({"op":"ping"})).is_err(),
                "A backend appeared while UAC was pending; reconnect before retrying"
            );
            if ep.exists() {
                let old: peripheral_repl::protocol::Endpoint =
                    serde_json::from_slice(&std::fs::read(ep)?)?;
                anyhow::ensure!(
                    !crate::native::process_alive(old.pid),
                    "Existing backend is alive but unresponsive; no duplicate started"
                );
                std::fs::remove_file(ep)?;
            }
        } else {
            let ping = request(ep, json!({"op":"ping"}))?;
            anyhow::ensure!(
                ping["runtime_id"] == args[2],
                "Backend changed while UAC was pending; original process was not stopped"
            );
            anyhow::ensure!(
                Path::new(ping["root"].as_str().context("backend workspace")?).canonicalize()?
                    == root,
                "Backend workspace changed"
            );
            super::lifecycle::stop(ep)?;
        }
        peripheral_repl::protocol::serve(runtime, ep, root, None)
    })();
    if let Err(error) = &result {
        // The endpoint directory already has a private, same-user ACL. No secrets are logged.
        if ep.parent().is_some_and(Path::is_dir) {
            let _ = std::fs::write(
                &report,
                json!({"error":format!("{error:#}"),"pid":std::process::id()}).to_string(),
            );
        }
    }
    result
}

// Windows CommandLineToArgvW / CRT quoting; all arguments, including paths, are literal.
pub(super) fn quote(value: &str) -> String {
    let mut out = String::from("\"");
    let mut slashes = 0;
    for c in value.chars() {
        if c == '\\' {
            slashes += 1;
            continue;
        }
        if c == '"' {
            out.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
        } else {
            out.extend(std::iter::repeat_n('\\', slashes));
        }
        out.push(c);
        slashes = 0;
    }
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn literal_windows_arguments_round_trip() {
        use windows::Win32::{
            Foundation::{HLOCAL, LocalFree},
            UI::Shell::CommandLineToArgvW,
        };
        use windows::core::HSTRING;
        let input = [
            "app.exe",
            "C:\\路径 with spaces\\",
            "a\"b",
            "%TEMP%",
            "& echo bad",
            "",
            "$x `y`",
        ];
        let command = HSTRING::from(input.iter().map(|s| quote(s)).collect::<Vec<_>>().join(" "));
        unsafe {
            let mut count = 0;
            let argv = CommandLineToArgvW(&command, &mut count);
            assert!(!argv.is_null());
            let actual = (0..count as usize)
                .map(|i| (*argv.add(i)).to_string().unwrap())
                .collect::<Vec<_>>();
            let _ = LocalFree(Some(HLOCAL(argv.cast())));
            assert_eq!(actual, input);
        }
    }
}
