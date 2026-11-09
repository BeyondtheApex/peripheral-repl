//! Peripheral extension: bounded frame diagnostics and opt-in GPU consumers.
//! Observers run on the renderer thread and must never wait for another process.
use std::{cell::RefCell, collections::HashMap};
use windows::Win32::Graphics::Direct3D11::{ID3D11Device,ID3D11DeviceContext,ID3D11Texture2D};

// GPUI's invalidator already exposes frame demand. Avoid waking every HWND on
// every VSync while the application is idle. The timeout only checks shutdown
// and device health; it does not invalidate windows.
static DEMAND:std::sync::LazyLock<(std::sync::Mutex<std::collections::HashSet<usize>>,std::sync::Condvar)>=std::sync::LazyLock::new(Default::default);
pub(crate) fn request(hwnd:usize){let(mut pending,signal)= (DEMAND.0.lock().unwrap_or_else(|e|e.into_inner()),&DEMAND.1);pending.insert(hwnd);signal.notify_one();}
pub(crate) fn wait_for_demand()->bool{let pending=DEMAND.0.lock().unwrap_or_else(|e|e.into_inner());let (pending,_)=DEMAND.1.wait_timeout_while(pending,std::time::Duration::from_secs(1),|p|p.is_empty()).unwrap_or_else(|e|e.into_inner());!pending.is_empty()}
pub(crate) fn take_demand()->std::collections::HashSet<usize>{std::mem::take(&mut *DEMAND.0.lock().unwrap_or_else(|e|e.into_inner()))}

pub struct Frame<'a> {pub device:&'a ID3D11Device,pub context:&'a ID3D11DeviceContext,pub texture:&'a ID3D11Texture2D}
type Observer=Box<dyn FnMut(Frame<'_>)>;
#[derive(Default,Clone,Debug)]
pub struct Stats {pub frames:u64,pub cpu_submit_us:u64,pub max_cpu_submit_us:u64}
thread_local! {
    static STATS:RefCell<HashMap<usize,Stats>>=RefCell::new(HashMap::new());
    static OBSERVERS:RefCell<HashMap<usize,Observer>>=RefCell::new(HashMap::new());
    static EXTERNAL:RefCell<std::collections::HashSet<usize>>=RefCell::new(Default::default());
}
pub fn external_only(hwnd:usize){EXTERNAL.with_borrow_mut(|s|{s.insert(hwnd);});}
pub(crate) fn is_external(hwnd:usize)->bool{EXTERNAL.with_borrow(|s|s.contains(&hwnd))}
pub fn observe(hwnd:usize,observer:impl FnMut(Frame<'_>)+'static){OBSERVERS.with_borrow_mut(|map|{map.insert(hwnd,Box::new(observer));});}
pub fn remove(hwnd:usize){OBSERVERS.with_borrow_mut(|m|{m.remove(&hwnd);});STATS.with_borrow_mut(|m|{m.remove(&hwnd);});EXTERNAL.with_borrow_mut(|m|{m.remove(&hwnd);});}
pub fn stats(hwnd:usize)->Stats {STATS.with_borrow(|m|m.get(&hwnd).cloned().unwrap_or_default())}
pub(crate) fn notify(hwnd:usize,frame:Frame<'_>){OBSERVERS.with_borrow_mut(|m|{if let Some(observer)=m.get_mut(&hwnd){observer(frame);}});}
pub(crate) fn record(hwnd:usize,cpu_us:u64){STATS.with_borrow_mut(|m|{let s=m.entry(hwnd).or_default();s.frames+=1;s.cpu_submit_us+=cpu_us;s.max_cpu_submit_us=s.max_cpu_submit_us.max(cpu_us);});}

pub fn unobserve(hwnd:usize){OBSERVERS.with_borrow_mut(|m|{m.remove(&hwnd);});}
