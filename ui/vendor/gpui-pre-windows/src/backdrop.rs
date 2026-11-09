//! One owner for native backdrop state. Transparent must never enable
//! ACCENT_ENABLE_TRANSPARENTGRADIENT (2): that paints the Windows accent color.
use anyhow::{Context, Result, ensure};
use gpui::WindowBackgroundAppearance;
use std::{cell::RefCell,collections::HashMap,ffi::c_void};
use windows::{core::{s,w,BOOL},Win32::{Foundation::HWND,Graphics::Dwm::*,System::LibraryLoader::{GetModuleHandleW,GetProcAddress},UI::Controls::MARGINS}};

#[derive(Clone,Debug)]
pub struct Status {pub path:&'static str,pub error:Option<String>}
thread_local! {static STATUS:RefCell<HashMap<usize,Status>>=RefCell::new(HashMap::new());}
pub fn status(hwnd:usize)->Option<Status>{STATUS.with(|s|s.borrow().get(&hwnd).cloned())}
pub fn forget(hwnd:usize){STATUS.with(|s|{s.borrow_mut().remove(&hwnd);});}

#[repr(C)]
struct AccentPolicy {state:u32,flags:u32,color:u32,animation:u32}
#[repr(C)]
struct CompositionAttribute {attribute:u32,data:*mut c_void,bytes:usize}
fn accent(hwnd:HWND,state:u32)->Result<()> {
    unsafe {
        let module=GetModuleHandleW(w!("user32.dll"))?;
        let address=GetProcAddress(module,s!("SetWindowCompositionAttribute")).context("composition attributes unavailable")?;
        let set:unsafe extern "system" fn(HWND,*mut CompositionAttribute)->BOOL=std::mem::transmute(address);
        // Acrylic needs nonzero alpha. Neutral 1/255 tint leaves coloring to
        // the GPUI layer, and avoids the system accent-color gradient entirely.
        let mut policy=AccentPolicy{state,flags:0,color:if state==4{0x01000000}else{0},animation:0};
        let mut data=CompositionAttribute{attribute:19,data:(&mut policy as *mut AccentPolicy).cast(),bytes:std::mem::size_of::<AccentPolicy>()};
        ensure!(set(hwnd,&mut data).as_bool(),"SetWindowCompositionAttribute({state}) failed");
    }
    Ok(())
}
fn dwm(hwnd:HWND,value:DWM_SYSTEMBACKDROP_TYPE)->windows::core::Result<()> {
    unsafe{DwmSetWindowAttribute(hwnd,DWMWA_SYSTEMBACKDROP_TYPE,(&value as *const DWM_SYSTEMBACKDROP_TYPE).cast(),std::mem::size_of_val(&value) as u32)}
}
fn frame(hwnd:HWND,glass:bool)->windows::core::Result<()> {
    let value=if glass{-1}else{0};
    unsafe{DwmExtendFrameIntoClientArea(hwnd,&MARGINS{cxLeftWidth:value,cxRightWidth:value,cyTopHeight:value,cyBottomHeight:value})}
}
pub(crate) fn apply(hwnd:HWND,appearance:WindowBackgroundAppearance) {
    let path=match appearance {
        WindowBackgroundAppearance::Opaque=>"solid",
        WindowBackgroundAppearance::Transparent=>"transparent",
        WindowBackgroundAppearance::Blurred=>"acrylic",
        WindowBackgroundAppearance::MicaBackdrop=>"mica",
        WindowBackgroundAppearance::MicaAltBackdrop=>"mica_alt",
    };
    let result=(||->Result<()> {
        accent(hwnd,0)?;
        // Clearing an unsupported backdrop attribute is harmless on older
        // Windows. Enabling Mica below still checks its own HRESULT.
        let _=dwm(hwnd,DWMSBT_NONE);
        frame(hwnd,false)?;
        match appearance {
            WindowBackgroundAppearance::Blurred=>accent(hwnd,4)?,
            WindowBackgroundAppearance::MicaBackdrop|WindowBackgroundAppearance::MicaAltBackdrop=>{
                dwm(hwnd,if appearance==WindowBackgroundAppearance::MicaBackdrop{DWMSBT_MAINWINDOW}else{DWMSBT_TABBEDWINDOW})?;
                frame(hwnd,true)?;
            },
            _=>{},
        }
        Ok(())
    })();
    if result.is_err(){let _=accent(hwnd,0);let _=dwm(hwnd,DWMSBT_NONE);let _=frame(hwnd,false);}
    let status=Status{path:if result.is_ok(){path}else{"fallback"},error:result.err().map(|e|e.to_string())};
    STATUS.with(|s|{s.borrow_mut().insert(hwnd.0 as usize,status);});
}
