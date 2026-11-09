use super::*;
struct Consumer {
    dir: PathBuf,
    generation: u64,
    graphics: Option<Graphics>,
    frames: u64,
    error: Option<String>,
    input: std::sync::mpsc::SyncSender<Value>,
}
struct Graphics {
    _device: ID3D11Device,
    context: ID3D11DeviceContext,
    swap: IDXGISwapChain1,
    _composition: IDCompositionDevice,
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
    slots: Vec<(ID3D11Texture2D, IDXGIKeyedMutex)>,
}
impl Graphics {
    fn new(hwnd: HWND, meta: &Value) -> Result<Self> {
        unsafe {
            let factory: IDXGIFactory2 = CreateDXGIFactory1()?;
            let mut selected = None;
            for index in 0..32 {
                let Ok(adapter) = factory.EnumAdapters1(index) else {
                    break;
                };
                let d = adapter.GetDesc1()?;
                if d.AdapterLuid.LowPart == meta["adapter_low"].as_u64().unwrap_or(0) as u32
                    && d.AdapterLuid.HighPart == meta["adapter_high"].as_i64().unwrap_or(0) as i32
                {
                    selected = Some(adapter);
                    break;
                }
            }
            let adapter = selected.context("producer GPU adapter not found")?;
            let (mut device, mut context) = (None, None);
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
            let device = device.context("D3D device")?;
            let context = context.context("D3D context")?;
            let width = meta["width"].as_u64().context("width")? as u32;
            let height = meta["height"].as_u64().context("height")? as u32;
            anyhow::ensure!(
                width > 0 && height > 0 && width <= 4096 && height <= 2160,
                "frame bounds exceeded"
            );
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: width,
                Height: height,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                ..Default::default()
            };
            let swap = factory.CreateSwapChainForComposition(&device, &desc, None)?;
            let dxgi: IDXGIDevice = device.cast()?;
            let composition: IDCompositionDevice = DCompositionCreateDevice(&dxgi)?;
            let target = composition.CreateTargetForHwnd(hwnd, true)?;
            let visual = composition.CreateVisual()?;
            visual.SetContent(&swap)?;
            target.SetRoot(&visual)?;
            composition.Commit()?;
            let d1: ID3D11Device1 = device.cast()?;
            let mut slots = Vec::new();
            for name in meta["names"].as_array().context("slots")?.iter().take(2) {
                let texture: ID3D11Texture2D = d1.OpenSharedResourceByName(
                    &HSTRING::from(name.as_str().context("name")?),
                    DXGI_SHARED_RESOURCE_READ.0 | DXGI_SHARED_RESOURCE_WRITE.0,
                )?;
                let mutex = texture.cast()?;
                slots.push((texture, mutex));
            }
            anyhow::ensure!(slots.len() == 2, "two slots required");
            SetWindowPos(
                hwnd,
                None,
                700,
                150,
                width as i32,
                height as i32,
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )?;
            Ok(Self {
                _device: device,
                context,
                swap,
                _composition: composition,
                _target: target,
                _visual: visual,
                slots,
            })
        }
    }
    fn frame(&self, index: usize) -> Result<bool> {
        unsafe {
            let (texture, mutex) = self.slots.get(index).context("slot index")?;
            let hr = (Interface::vtable(mutex).AcquireSync)(Interface::as_raw(mutex), 1, 0);
            if hr.0 != 0 {
                return Ok(false);
            }
            let result = (|| -> Result<()> {
                let back: ID3D11Texture2D = self.swap.GetBuffer(0)?;
                self.context.CopyResource(&back, texture);
                Ok(())
            })();
            let release = mutex.ReleaseSync(0);
            self.context.Flush();
            result?;
            release?;
            self.swap.Present(0, DXGI_PRESENT(0)).ok()?;
            Ok(true)
        }
    }
}
impl Consumer {
    fn frame(&mut self, hwnd: HWND, index: usize, generation: u64) -> Result<()> {
        if self.generation != generation {
            let meta: Value = serde_json::from_slice(&std::fs::read(self.dir.join("frame.json"))?)?;
            // One DComp target per HWND; release it before constructing its replacement.
            self.graphics = None;
            self.graphics = Some(Graphics::new(hwnd, &meta)?);
            self.generation = meta["generation"].as_u64().context("generation")?;
        }
        if self.generation == generation
            && self.graphics.as_ref().context("graphics")?.frame(index)?
        {
            self.frames += 1;
            self.error = None;
        }
        Ok(())
    }
    fn report(&self, hwnd: HWND) {
        let _=std::fs::write(self.dir.join("consumer.json"),json!({"pid":std::process::id(),"hwnd":hwnd.0 as usize,"frames":self.frames,"generation":self.generation,"error":self.error,"gpu_copies_per_frame":1,"cpu_pixel_copies":0}).to_string());
    }
}
unsafe extern "system" fn consumer_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        consumer_proc_inner(hwnd, msg, w, l)
    }))
    .unwrap_or_else(|_| unsafe { DefWindowProcW(hwnd, msg, w, l) })
}
unsafe fn consumer_proc_inner(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = unsafe { &*(l.0 as *const CREATESTRUCTW) };
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        return LRESULT(1);
    }
    let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Consumer;
    if !p.is_null() {
        let state = unsafe { &mut *p };
        if msg == FRAME_MESSAGE {
            let previous = state.generation;
            if let Err(e) = state.frame(hwnd, w.0, l.0 as u64) {
                state.error = Some(format!("{e:#}"));
            }
            if previous != state.generation || state.error.is_some() {
                state.report(hwnd);
            }
            return LRESULT(0);
        }
        if msg == FRAME_MESSAGE + 1 {
            state.report(hwnd);
            return LRESULT(0);
        }
        if [
            WM_MOUSEMOVE,
            WM_LBUTTONDOWN,
            WM_LBUTTONUP,
            WM_RBUTTONDOWN,
            WM_RBUTTONUP,
            WM_MOUSEWHEEL,
        ]
        .contains(&msg)
        {
            let mut point = POINT {
                x: (l.0 as u16 as i16) as i32,
                y: ((l.0 as u32 >> 16) as u16 as i16) as i32,
            };
            if msg == WM_MOUSEWHEEL {
                unsafe {
                    let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut point);
                }
            }
            let (x, y) = (point.x, point.y);
            let _ = state
                .input
                .try_send(json!({"message":msg,"x":x,"y":y,"wparam":w.0}));
            return LRESULT(0);
        }
        if msg == WM_MOUSEACTIVATE {
            return LRESULT(MA_NOACTIVATE as isize);
        }
        if msg == WM_DESTROY {
            unsafe {
                PostQuitMessage(0);
            }
            return LRESULT(0);
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, w, l) }
}

pub fn run_consumer(dir: &Path, source: &str) -> Result<()> {
    let session: Value = serde_json::from_slice(&std::fs::read(
        std::path::PathBuf::from(std::env::var("LOCALAPPDATA")?)
            .join("peripheral-repl/ui/session.json"),
    )?)?;
    let (tx, rx) = std::sync::mpsc::sync_channel::<Value>(64);
    let source = source.to_owned();
    let directory = dir.to_owned();
    std::thread::spawn(move || {
        use std::io::{BufRead, Write};
        for event in rx {
            let Ok(mut stream) =
                std::net::TcpStream::connect(session["address"].as_str().unwrap_or(""))
            else {
                break;
            };
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(1)));
            let request = json!({"op":"pointer","surface":source,"event":event,"directory":directory,"token":session["token"]});
            if writeln!(stream, "{request}").is_ok() {
                let mut line = String::new();
                let _ = std::io::BufReader::new(stream).read_line(&mut line);
            }
        }
    });
    let mut state = Box::new(Consumer {
        dir: dir.to_owned(),
        generation: 0,
        graphics: None,
        frames: 0,
        error: None,
        input: tx,
    });
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(consumer_proc),
            hInstance: instance.into(),
            lpszClassName: w!("PeripheralTextureConsumer"),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        anyhow::ensure!(RegisterClassW(&class) != 0, "register texture window");
        let hwnd = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            w!("PeripheralTextureConsumer"),
            w!("Peripheral · GPU texture host"),
            WS_POPUP | WS_VISIBLE,
            700,
            150,
            540,
            400,
            None,
            None,
            Some(instance.into()),
            Some(&mut *state as *mut Consumer as _),
        )?;
        state.report(hwnd);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}
