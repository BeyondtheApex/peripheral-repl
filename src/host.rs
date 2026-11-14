//! Process facts, shared by Lua, IPC and the GUI. Never changes privileges.
use serde_json::{Value, json};

pub fn status() -> Value {
    #[cfg(windows)]
    {
        static VALUE: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
        VALUE.get_or_init(|| match elevation() {
            Ok(elevated) => json!({"platform":"windows","elevated":elevated,"pid":std::process::id()}),
            Err(e) => json!({"platform":"windows","elevated":null,"pid":std::process::id(),"error":e.to_string()}),
        }).clone()
    }
    #[cfg(not(windows))]
    json!({"platform":std::env::consts::OS,"elevated":null,"pid":std::process::id()})
}

#[cfg(windows)]
pub fn elevation() -> windows::core::Result<bool> {
    use windows::Win32::{
        Foundation::{CloseHandle, HANDLE},
        Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
        let mut value = TOKEN_ELEVATION::default();
        let mut size = 0;
        let result = GetTokenInformation(
            token,
            TokenElevation,
            Some((&mut value as *mut TOKEN_ELEVATION).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        );
        let _ = CloseHandle(token);
        result.map(|_| value.TokenIsElevated != 0)
    }
}

#[cfg(windows)]
pub fn user_sid() -> windows::core::Result<String> {
    use windows::{
        Win32::{
            Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree},
            Security::{
                Authorization::ConvertSidToStringSidW, GetTokenInformation, TOKEN_QUERY,
                TOKEN_USER, TokenUser,
            },
            System::Threading::{GetCurrentProcess, OpenProcessToken},
        },
        core::PWSTR,
    };
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
        let result = (|| -> windows::core::Result<String> {
            let mut size = 0;
            let _ = GetTokenInformation(token, TokenUser, None, 0, &mut size);
            let mut buffer = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
            GetTokenInformation(
                token,
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                size,
                &mut size,
            )?;
            let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
            let mut text = PWSTR::null();
            ConvertSidToStringSidW(user.User.Sid, &mut text)?;
            let result = text.to_string().map_err(|_| {
                windows::core::Error::from_hresult(windows::Win32::Foundation::E_INVALIDARG)
            });
            let _ = LocalFree(Some(HLOCAL(text.0.cast())));
            result
        })();
        let _ = CloseHandle(token);
        result
    }
}
