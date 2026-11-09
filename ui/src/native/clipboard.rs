//! Text clipboard writes with a real owner HWND and observable Win32 errors.
use super::*;
use windows::{
    Win32::System::{
        DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
        Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
        Ole::CF_UNICODETEXT,
    },
    core::Owned,
};

pub fn write_text(owner: usize, text: &str) -> Result<()> {
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseClipboard();
            }
        }
    }
    // OpenClipboard(NULL) + EmptyClipboard leaves no owner and can make
    // SetClipboardData fail. Always use the application's real HWND.
    unsafe { OpenClipboard(Some(handle(owner))) }.context("clipboard is busy")?;
    let _guard = Guard;
    let wide = text
        .replace('\0', " ")
        .encode_utf16()
        .chain(Some(0))
        .collect::<Vec<_>>();
    unsafe {
        let memory = Owned::new(GlobalAlloc(
            GMEM_MOVEABLE,
            std::mem::size_of_val(wide.as_slice()),
        )?);
        let destination = GlobalLock(*memory);
        anyhow::ensure!(!destination.is_null(), "clipboard memory unavailable");
        std::ptr::copy_nonoverlapping(wide.as_ptr(), destination.cast(), wide.len());
        let _ = GlobalUnlock(*memory);
        EmptyClipboard()?;
        SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(memory.0)))?;
        std::mem::forget(memory); // Ownership transferred to Windows.
    }
    Ok(())
}
