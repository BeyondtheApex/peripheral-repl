//! Launch the fixed HID helper without Windows' GUI startup cursor. This flag
//! belongs to STARTUPINFO, not CREATE_NO_WINDOW. Rust's stable Command does not
//! expose it. Keep native spawning narrow: one executable, one fixed argument,
//! inherited environment/current directory and an explicit three-handle list.
use std::{
    io,
    os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::ExitStatusExt,
    },
    path::Path,
    process::ExitStatus,
};
use windows::{
    Win32::{Foundation::*, System::Threading::*},
    core::{HSTRING, PCWSTR, PWSTR},
};

pub(super) struct Process(OwnedHandle);
impl Process {
    fn handle(&self) -> HANDLE {
        HANDLE(self.0.as_raw_handle())
    }
    pub fn kill(&mut self) -> io::Result<()> {
        if self.try_wait()?.is_none() {
            unsafe {
                TerminateProcess(self.handle(), 1)?;
            }
        }
        Ok(())
    }
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        match unsafe { WaitForSingleObject(self.handle(), 0) } {
            WAIT_OBJECT_0 => {
                let mut code = 0;
                unsafe {
                    GetExitCodeProcess(self.handle(), &mut code)?;
                }
                Ok(Some(ExitStatus::from_raw(code)))
            }
            WAIT_TIMEOUT => Ok(None),
            _ => Err(io::Error::last_os_error()),
        }
    }
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        if unsafe { WaitForSingleObject(self.handle(), INFINITE) } != WAIT_OBJECT_0 {
            return Err(io::Error::last_os_error());
        }
        self.try_wait()?
            .ok_or_else(|| io::Error::other("worker not signaled"))
    }
    #[cfg(test)]
    pub fn id(&self) -> u32 {
        unsafe { GetProcessId(self.handle()) }
    }
}

fn inheritable(handle: &impl AsRawHandle) -> io::Result<OwnedHandle> {
    let mut copy = HANDLE::default();
    unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            HANDLE(handle.as_raw_handle()),
            GetCurrentProcess(),
            &mut copy,
            0,
            true,
            DUPLICATE_SAME_ACCESS,
        )?;
        Ok(OwnedHandle::from_raw_handle(copy.0))
    }
}
struct Attributes {
    // usize storage supplies pointer alignment and stays alive until after spawn.
    _storage: Vec<usize>,
    list: LPPROC_THREAD_ATTRIBUTE_LIST,
}
impl Attributes {
    fn new(handles: &[HANDLE]) -> io::Result<Self> {
        let mut bytes = 0;
        unsafe {
            let _ = InitializeProcThreadAttributeList(None, 1, None, &mut bytes);
        }
        if bytes == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut storage = vec![0usize; bytes.div_ceil(std::mem::size_of::<usize>())];
        let list = LPPROC_THREAD_ATTRIBUTE_LIST(storage.as_mut_ptr().cast());
        unsafe {
            InitializeProcThreadAttributeList(Some(list), 1, None, &mut bytes)?;
        }
        let attributes = Self {
            _storage: storage,
            list,
        };
        unsafe {
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                Some(handles.as_ptr().cast()),
                std::mem::size_of_val(handles),
                None,
                None,
            )?;
        }
        Ok(attributes)
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        unsafe {
            DeleteProcThreadAttributeList(self.list);
        }
    }
}

pub(super) fn spawn(executable: &Path) -> io::Result<(Process, io::PipeWriter, io::PipeReader)> {
    spawn_fixed(executable, " --native-hid-worker")
}

fn spawn_fixed(
    executable: &Path,
    fixed_args: &str,
) -> io::Result<(Process, io::PipeWriter, io::PipeReader)> {
    // Application path is explicit; no shell or argument interpolation. Quotes
    // are illegal in a Windows filename. Arguments here are fixed host code.
    let executable = HSTRING::from(executable.as_os_str());
    if executable.contains(&0) || executable.contains(&(b'"' as u16)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid worker executable",
        ));
    }
    let mut command = vec![b'"' as u16];
    command.extend_from_slice(&executable);
    command.push(b'"' as u16);
    command.extend(fixed_args.encode_utf16());
    command.push(0);
    let (input_read, input_write) = io::pipe()?;
    let (output_read, output_write) = io::pipe()?;
    let error = std::fs::OpenOptions::new().write(true).open("NUL")?;
    let child_in = inheritable(&input_read)?;
    let child_out = inheritable(&output_write)?;
    let child_err = inheritable(&error)?;
    let handles = [
        HANDLE(child_in.as_raw_handle()),
        HANDLE(child_out.as_raw_handle()),
        HANDLE(child_err.as_raw_handle()),
    ];
    let attributes = Attributes::new(&handles)?;
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES | STARTF_FORCEOFFFEEDBACK;
    startup.StartupInfo.hStdInput = handles[0];
    startup.StartupInfo.hStdOutput = handles[1];
    startup.StartupInfo.hStdError = handles[2];
    startup.lpAttributeList = attributes.list;
    let mut info = PROCESS_INFORMATION::default();
    unsafe {
        CreateProcessW(
            &executable,
            Some(PWSTR(command.as_mut_ptr())),
            None,
            None,
            true,
            CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT,
            None,
            PCWSTR::null(),
            &startup.StartupInfo,
            &mut info,
        )?;
        let _thread = OwnedHandle::from_raw_handle(info.hThread.0);
        let process = Process(OwnedHandle::from_raw_handle(info.hProcess.0));
        Ok((process, input_write, output_read))
    }
}

pub(super) fn feedback_disabled() -> bool {
    let mut info = STARTUPINFOW::default();
    unsafe {
        GetStartupInfoW(&mut info);
    }
    info.dwFlags.contains(STARTF_FORCEOFFFEEDBACK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, Write};
    #[test]
    fn child_probe() {
        // Only run the pipe fixture when selected as an isolated subprocess.
        if !std::env::args().any(|a| a == "--exact") {
            return;
        }
        assert!(feedback_disabled());
        for line in io::stdin().lock().lines() {
            println!("echo:{}", line.unwrap());
        }
    }
    #[test]
    fn quiet_launch_round_trips_pipes_closes_on_eof_and_reaps() {
        let executable = std::env::current_exe().unwrap();
        let (mut child, mut input, output) = spawn_fixed(
            &executable,
            " --exact native_hid::windows_worker::tests::child_probe --nocapture",
        )
        .unwrap();
        writeln!(input, "Unicode 设备 \"paths\" \\ pipes").unwrap();
        drop(input);
        let reader = std::thread::spawn(move || {
            io::BufReader::new(output)
                .lines()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
                .join("\n")
        });
        let until = std::time::Instant::now() + std::time::Duration::from_secs(8);
        while child.try_wait().unwrap().is_none() && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if child.try_wait().unwrap().is_none() {
            child.kill().unwrap();
            panic!("EOF did not reach worker");
        }
        assert!(child.wait().unwrap().success());
        assert!(
            reader
                .join()
                .unwrap()
                .contains("echo:Unicode 设备 \"paths\" \\ pipes")
        );
    }
}
