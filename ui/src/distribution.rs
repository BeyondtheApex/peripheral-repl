//! Immutable two-file UI distribution; sessions never contain executable code.
use anyhow::{Context, Result};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{ffi::CStr, path::PathBuf};
use windows::{
    Win32::System::LibraryLoader::*,
    core::{HSTRING, PCSTR},
};

pub const ABI: u32 = 2;
pub fn expected_build() -> &'static str {
    option_env!("PERIPHERAL_NATIVE_BUILD").unwrap_or("")
}
pub fn dll_path() -> Result<PathBuf> {
    Ok(std::env::current_exe()?
        .parent()
        .context("executable directory")?
        .join("peripheral-taskbar-hook.dll"))
}
pub fn hash(path: &std::path::Path) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(std::fs::read(path)?)))
}
pub fn verified_dll() -> Result<PathBuf> {
    let path = dll_path()?;
    let expected = option_env!("PERIPHERAL_NATIVE_SHA256").unwrap_or("");
    anyhow::ensure!(
        !expected.is_empty(),
        "native adapter not bound to this build; run build.ps1"
    );
    let actual = hash(&path).with_context(|| format!("native DLL missing or unreadable: {}. Check installation and security protection history", path.display()))?;
    anyhow::ensure!(
        actual == expected,
        "native DLL does not match this EXE: {}. Restore the matching release pair",
        path.display()
    );
    std::fs::canonicalize(path).context("resolve physical native DLL")
}
pub fn report() -> Result<Value> {
    let exe = std::env::current_exe()?;
    let dll = dll_path()?;
    let verified = verified_dll();
    let data = PathBuf::from(std::env::var("LOCALAPPDATA")?).join("peripheral-repl/ui");
    Ok(
        json!({"executable":exe,"dll":dll,"physical_dll":std::fs::canonicalize(&dll).ok(),
        "dll_exists":dll.is_file(),"dll_sha256":hash(&dll).ok(),"expected_sha256":option_env!("PERIPHERAL_NATIVE_SHA256"),
        "native_abi":ABI,"native_build":expected_build(),"native_valid":verified.is_ok(),"native_error":verified.err().map(|e|format!("{e:#}")),
        "data_directory":data,"physical_data_directory":std::fs::canonicalize(&data).ok(),
        "architecture":std::env::consts::ARCH,"taskbar_support":"experimental Windows 11 x64; private XAML layout probe required"}),
    )
}
pub fn write_report() -> Result<PathBuf> {
    let dir = PathBuf::from(std::env::var("LOCALAPPDATA")?).join("peripheral-repl/ui");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("installation.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&report()?)?)?;
    Ok(path)
}
pub fn run_helper(mode: &str, args: &[String]) -> Result<i32> {
    // Only this explicitly launched helper process loads native code locally.
    // Main UI/overlay remain available if the optional DLL was quarantined.
    let dll = verified_dll()?;
    unsafe {
        let module = LoadLibraryExW(
            &HSTRING::from(dll.as_os_str()),
            None,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
        )?;
        let symbol = |name: &'static [u8]| {
            GetProcAddress(module, PCSTR(name.as_ptr())).context("native export missing")
        };
        let abi: unsafe extern "system" fn() -> u32 =
            std::mem::transmute(symbol(b"PeripheralNativeAbi\0")?);
        let build: unsafe extern "system" fn() -> *const std::ffi::c_char =
            std::mem::transmute(symbol(b"PeripheralNativeBuild\0")?);
        anyhow::ensure!(
            abi() == ABI && CStr::from_ptr(build()).to_str()? == expected_build(),
            "native ABI/build mismatch"
        );
        match mode {
            "--xaml-consumer" => {
                let dir = HSTRING::from(args.first().context("bridge directory")?.as_str());
                let run: unsafe extern "system" fn(*const u16) -> i32 =
                    std::mem::transmute(symbol(b"PeripheralRunXamlHost\0")?);
                Ok(run(dir.as_ptr()))
            }
            "--taskbar-loader" => {
                let pid: u32 = args.first().context("Explorer PID")?.parse()?;
                let config = HSTRING::from(args.get(1).context("session configuration")?.as_str());
                let dll = HSTRING::from(dll.as_os_str());
                let run: unsafe extern "system" fn(u32, *const u16, *const u16) -> i32 =
                    std::mem::transmute(symbol(b"PeripheralRunTaskbarLoader\0")?);
                Ok(run(pid, dll.as_ptr(), config.as_ptr()))
            }
            "--native-check" => Ok(0),
            _ => anyhow::bail!("unsupported native helper mode"),
        }
        // Keep module loaded until process exit: no callbacks into unloaded code.
    }
}
