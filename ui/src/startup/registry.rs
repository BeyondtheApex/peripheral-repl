//! Per-user Windows logon registration. No service, elevation or shell injection.
use anyhow::{Result, ensure};
use std::{os::windows::ffi::OsStrExt, path::Path};
use windows::{
    Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND},
        System::Registry::*,
    },
    core::HSTRING,
};

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const NAME: &str = "Peripheral";
struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}
fn read(path: &str, name: &str, kind: REG_ROUTINE_FLAGS) -> Result<Option<Vec<u8>>> {
    let mut bytes = vec![0u8; 32768];
    let mut length = bytes.len() as u32;
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(path),
            &HSTRING::from(name),
            kind,
            None,
            Some(bytes.as_mut_ptr().cast()),
            Some(&mut length),
        )
    };
    if result == ERROR_FILE_NOT_FOUND || result == ERROR_PATH_NOT_FOUND {
        return Ok(None);
    }
    result.ok()?;
    bytes.truncate(length as usize);
    Ok(Some(bytes))
}
fn command(exe: &Path) -> Result<String> {
    let path = exe
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("executable path is not Unicode"))?;
    ensure!(
        exe.is_absolute() && !path.contains(['"', '\0']),
        "invalid executable path"
    );
    Ok(format!("\"{path}\" --autostart"))
}
fn write(path: &str, name: &str, value: Option<&str>) -> Result<()> {
    let mut key = HKEY::default();
    unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &HSTRING::from(path),
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
    }
    .ok()?;
    let key = Key(key);
    if let Some(value) = value {
        let bytes = std::ffi::OsStr::new(value)
            .encode_wide()
            .chain(Some(0))
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        unsafe { RegSetValueExW(key.0, &HSTRING::from(name), None, REG_SZ, Some(&bytes)) }.ok()?;
    } else {
        let result = unsafe { RegDeleteValueW(key.0, &HSTRING::from(name)) };
        if result != ERROR_FILE_NOT_FOUND {
            result.ok()?;
        }
    }
    Ok(())
}
pub fn status() -> Result<(bool, bool, bool)> {
    let entry = read(RUN, NAME, RRF_RT_REG_SZ)?.map(|bytes| {
        let wide = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|v| *v != 0)
            .collect::<Vec<_>>();
        String::from_utf16_lossy(&wide)
    });
    let blocked = read(APPROVED, NAME, RRF_RT_REG_BINARY)?
        .and_then(|v| v.first().copied())
        .is_some_and(|v| matches!(v, 3 | 7));
    let matching = entry.as_deref() == Some(command(&std::env::current_exe()?)?.as_str());
    Ok((entry.is_some(), matching, blocked))
}
pub fn set(enabled: bool) -> Result<()> {
    let value = enabled
        .then(|| command(&std::env::current_exe()?))
        .transpose()?;
    write(RUN, NAME, value.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registration_round_trips_quoted_unicode_paths_and_removes_only_its_value() {
        let path = format!(r"Software\PeripheralTest-{}", uuid::Uuid::new_v4());
        let value = command(Path::new(r"C:\Program Files\设备\peripheral-ui.exe")).unwrap();
        assert_eq!(
            value,
            "\"C:\\Program Files\\设备\\peripheral-ui.exe\" --autostart"
        );
        write(&path, "test", Some(&value)).unwrap();
        write(&path, "keep", Some("unchanged")).unwrap();
        let bytes = read(&path, "test", RRF_RT_REG_SZ).unwrap().unwrap();
        assert_eq!(
            bytes,
            value
                .encode_utf16()
                .chain(Some(0))
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>()
        );
        write(&path, "test", None).unwrap();
        write(&path, "test", None).unwrap();
        assert!(read(&path, "test", RRF_RT_REG_SZ).unwrap().is_none());
        assert!(read(&path, "keep", RRF_RT_REG_SZ).unwrap().is_some());
        unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(path)) }
            .ok()
            .unwrap();
    }
}
