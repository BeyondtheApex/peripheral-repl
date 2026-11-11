//! Native HID calls run in a disposable subprocess, never in the Lua host.
use crate::{
    bindings::{hex, unhex},
    error::Fault,
};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{Mutex, mpsc},
    time::Duration,
};

struct Process {
    child: Child,
    input: ChildStdin,
    output: mpsc::Receiver<Value>,
    dead: bool,
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
pub struct NativeHid {
    process: Mutex<Process>,
    input_reports: bool,
}
fn descriptor_has_input(bytes: &[u8]) -> Option<bool> {
    let parsed = crate::hid_descriptor::parse(bytes).ok()?;
    let reports = parsed["reports"].as_array()?;
    // An empty or unsupported descriptor is unknown, not proof of no Input.
    (!reports.is_empty()).then(|| reports.iter().any(|r| r["type"] == "input"))
}
impl NativeHid {
    fn spawn() -> Result<Self, Fault> {
        let mut executable =
            std::env::current_exe().map_err(|e| Fault::new("backend", "hid.worker", e))?;
        if executable.parent().is_some_and(|p| p.ends_with("deps")) {
            executable = executable
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join(if cfg!(windows) {
                    "peripheral-repl.exe"
                } else {
                    "peripheral-repl"
                });
        }
        let mut command = Command::new(executable);
        command.arg("--native-hid-worker");
        Self::spawn_command(command)
    }
    fn spawn_command(mut command: Command) -> Result<Self, Fault> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command
            .spawn()
            .map_err(|e| Fault::new("backend", "hid.worker", e))?;
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(output);
            loop {
                let mut line = String::new();
                if reader
                    .by_ref()
                    .take(1048577)
                    .read_line(&mut line)
                    .unwrap_or(0)
                    == 0
                    || line.len() > 1048576
                {
                    break;
                }
                let Ok(value) = serde_json::from_str(&line) else {
                    break;
                };
                if tx.send(value).is_err() {
                    break;
                }
            }
        });
        let h = Self {
            input_reports: true,
            process: Mutex::new(Process {
                child,
                input,
                output: rx,
                dead: false,
            }),
        };
        Ok(h)
    }
    pub fn open(path: &str) -> Result<Self, Fault> {
        let mut h = Self::spawn()?;
        let result = h.call(json!({"op":"open","path":path}))?;
        h.input_reports = result["input_reports"].as_bool().unwrap_or(true);
        Ok(h)
    }
    pub fn has_input_reports(&self) -> bool {
        self.input_reports
    }
    pub fn enumerate(filter: Option<&Value>) -> Result<Value, Fault> {
        Self::spawn()?.call(json!({"op":"enumerate","filter":filter}))
    }
    pub fn call(&self, request: Value) -> Result<Value, Fault> {
        let mut p = self.process.lock().unwrap();
        if p.dead {
            return Err(Fault::new(
                "disconnected",
                "hid.worker",
                "native worker terminated; reconnect required",
            ));
        }
        let sent = serde_json::to_writer(&mut p.input, &request)
            .map_err(std::io::Error::other)
            .and_then(|_| p.input.write_all(b"\n"))
            .and_then(|_| p.input.flush());
        if let Err(e) = sent {
            p.dead = true;
            let _ = p.child.kill();
            return Err(Fault::new("disconnected", "hid.worker", e));
        }
        match p.output.recv_timeout(Duration::from_millis(1500)) {
            Ok(v) if v["ok"] == true => Ok(v["result"].clone()),
            Ok(v) => Err(serde_json::from_value(v["error"].clone())
                .unwrap_or_else(|_| Fault::new("backend", "hid.worker", "invalid worker error"))),
            Err(e) => {
                p.dead = true;
                let _ = p.child.kill();
                let _ = p.child.wait();
                Err(Fault::new(
                    "disconnected",
                    "hid.worker",
                    format!(
                        "native deadline or worker failure; worker terminated, reconnect required: {e}"
                    ),
                ))
            }
        }
    }
    fn read(&self, op: &str, data: &mut [u8], timeout: i32) -> Result<usize, Fault> {
        let result =
            self.call(json!({"op":op,"wire":hex(data),"length":data.len(),"timeout_ms":timeout}))?;
        let bytes = unhex(result["hex"].as_str().unwrap_or(""))
            .map_err(|e| Fault::new("backend", op, e))?;
        if bytes.len() > data.len() {
            return Err(Fault::new(
                "short_buffer",
                op,
                "native response exceeds capacity",
            ));
        }
        data[..bytes.len()].copy_from_slice(&bytes);
        Ok(bytes.len())
    }
    pub fn read_timeout(&self, data: &mut [u8], timeout: i32) -> Result<usize, Fault> {
        if !self.input_reports {
            // Feature-only collections reject ReadFile on Windows. Keep their
            // control channel alive without polling an absent Input endpoint.
            std::thread::sleep(Duration::from_millis(timeout.clamp(1, 50) as u64));
            return Ok(0);
        }
        self.read("read_input", data, timeout)
    }
    pub fn get_input_report(&self, data: &mut [u8]) -> Result<usize, Fault> {
        self.read("get_input_report", data, 0)
    }
    pub fn get_report_descriptor(&self, data: &mut [u8]) -> Result<usize, Fault> {
        self.read("descriptor", data, 0)
    }
    pub fn get_feature_report(&self, data: &mut [u8]) -> Result<usize, Fault> {
        self.read("get_feature", data, 0)
    }
    pub fn write(&self, data: &[u8]) -> Result<usize, Fault> {
        Ok(
            self.call(json!({"op":"write_output","wire":hex(data)}))?["length"]
                .as_u64()
                .unwrap_or(0) as usize,
        )
    }
    pub fn report(&self, op: &str, data: &[u8]) -> Result<usize, Fault> {
        Ok(self.call(json!({"op":op,"wire":hex(data)}))?["length"]
            .as_u64()
            .unwrap_or(0) as usize)
    }
}

pub fn serve() -> anyhow::Result<()> {
    use std::io::Read;
    let mut device = None;
    let mut input = BufReader::new(std::io::stdin());
    let mut output = std::io::stdout().lock();
    loop {
        let mut line = String::new();
        if input.by_ref().take(32769).read_line(&mut line)? == 0 {
            break;
        }
        anyhow::ensure!(line.len() <= 32768, "worker command too large");
        let request: Value = serde_json::from_str(&line)?;
        let result = (|| -> anyhow::Result<Value> {
            let op = request["op"].as_str().unwrap_or("");
            if op == "enumerate" {
                return crate::bindings::enumerate_local(
                    request.get("filter").filter(|v| !v.is_null()),
                );
            }
            if op == "open" {
                anyhow::ensure!(device.is_none(), "already open");
                let path = std::ffi::CString::new(
                    request["path"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("missing path"))?,
                )?;
                let handle = hidapi::HidApi::new()?.open_path(&path)?;
                let mut descriptor = [0; 4096];
                let input_reports = handle
                    .get_report_descriptor(&mut descriptor)
                    .ok()
                    .and_then(|n| descriptor_has_input(&descriptor[..n]));
                device = Some(handle);
                return Ok(json!({"input_reports":input_reports}));
            }
            let h = device.as_ref().ok_or_else(|| anyhow::anyhow!("not open"))?;
            let mut bytes = unhex(request["wire"].as_str().unwrap_or(""))?;
            anyhow::ensure!(!bytes.is_empty() && bytes.len() <= 4096, "invalid buffer");
            let n = match op {
                "read_input" => h.read_timeout(
                    &mut bytes,
                    request["timeout_ms"].as_i64().unwrap_or(10).clamp(0, 50) as i32,
                )?,
                "get_input_report" => h.get_input_report(&mut bytes)?,
                "get_feature" => h.get_feature_report(&mut bytes)?,
                "descriptor" => h.get_report_descriptor(&mut bytes)?,
                "write_output" => return Ok(json!({"length":h.write(&bytes)?})),
                "send_output_report" => {
                    h.send_output_report(&bytes)?;
                    return Ok(json!({"length":bytes.len()}));
                }
                "set_feature" => {
                    h.send_feature_report(&bytes)?;
                    return Ok(json!({"length":bytes.len()}));
                }
                _ => anyhow::bail!("unknown operation"),
            };
            anyhow::ensure!(
                n <= bytes.len(),
                "native report length exceeds requested capacity"
            );
            bytes.truncate(n);
            Ok(json!({"hex":hex(&bytes)}))
        })();
        let response = match result {
            Ok(v) => json!({"ok":true,"result":v}),
            Err(e) => json!({"ok":false,"error":Fault::new("backend","hid.native",e)}),
        };
        serde_json::to_writer(&mut output, &response)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    fn descriptor_distinguishes_feature_only_from_input_and_unknown() {
        assert_eq!(
            descriptor_has_input(&unhex("06ffff0900a10109001500250175089540b102c0").unwrap()),
            Some(false)
        );
        assert_eq!(
            descriptor_has_input(
                &unhex("0600ff090ea10185ba0901150026ff007508951f81000901150026ff007508951f9100c0")
                    .unwrap()
            ),
            Some(true)
        );
        assert_eq!(descriptor_has_input(&[]), None);
        assert_eq!(descriptor_has_input(&[0xfe]), None);
        assert_eq!(descriptor_has_input(&[0xa1, 1, 0xc0]), None);
    }
    #[test]
    fn feature_only_worker_never_receives_interrupt_reads() {
        let mut command = Command::new("powershell.exe");
        command.args(["-NoProfile", "-NonInteractive", "-Command",
            "$r=[Console]::ReadLine()|ConvertFrom-Json; if($r.op -eq 'set_feature'){[Console]::WriteLine('{\"ok\":true,\"result\":{\"length\":65}}')}else{[Console]::WriteLine('{\"ok\":false,\"error\":{\"code\":\"unexpected_read\",\"operation\":\"test\",\"message\":\"Feature-only collection has no Input\",\"retryable\":false}}')}"]);
        let mut worker = NativeHid::spawn_command(command).unwrap();
        worker.input_reports = false;
        assert_eq!(worker.read_timeout(&mut [0; 1024], 10).unwrap(), 0);
        assert_eq!(worker.report("set_feature", &[0; 65]).unwrap(), 65);
    }
    #[test]
    fn hung_native_process_is_killed_and_cannot_receive_more_commands() {
        let mut command = Command::new("powershell.exe");
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Console]::ReadLine() | Out-Null; Start-Sleep -Seconds 30",
        ]);
        let worker = NativeHid::spawn_command(command).unwrap();
        let pid = worker.process.lock().unwrap().child.id();
        let start = std::time::Instant::now();
        assert_eq!(
            worker.call(json!({"op":"test"})).unwrap_err().code,
            "disconnected"
        );
        assert!(start.elapsed() < Duration::from_secs(5));
        let mut process = worker.process.lock().unwrap();
        assert!(
            process.child.try_wait().unwrap().is_some(),
            "child {pid} must be gone"
        );
        drop(process);
        assert_eq!(
            worker.call(json!({"op":"test"})).unwrap_err().code,
            "disconnected"
        );
    }
}
