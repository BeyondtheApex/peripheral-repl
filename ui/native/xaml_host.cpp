// Independent validation host for the exact SwapChainPanel path intended for
// taskbar embedding. It never loads code into Explorer.
#ifdef PERIPHERAL_TASKBAR
#define DllGetClassObject PeripheralSdkDllGetClassObject
#define DllCanUnloadNow PeripheralSdkDllCanUnloadNow
#endif
#include <windows.h>
#include <inspectable.h>
#include <shellapi.h>
#include <d3d11_1.h>
#include <dxgi1_3.h>
#include <windows.ui.xaml.hosting.desktopwindowxamlsource.h>
#include <windows.ui.xaml.media.dxinterop.h>
#ifdef PERIPHERAL_TASKBAR
#include <ocidl.h>
#include <xamlom.h>
#undef DllGetClassObject
#undef DllCanUnloadNow
#endif
#undef GetCurrentTime
#include <winrt/Windows.Foundation.h>
#include <winrt/Windows.Foundation.Collections.h>
#include <winrt/Windows.Data.Json.h>
#include <winrt/Windows.UI.Xaml.h>
#include <winrt/Windows.UI.Xaml.Controls.h>
#include <winrt/Windows.UI.Xaml.Hosting.h>
#include <winrt/Windows.UI.Xaml.Input.h>
#include <winrt/Windows.UI.Input.h>
#include <filesystem>
#include <fstream>
#include <sstream>
#include <array>
#include <memory>
#include "native_build.h"

namespace xaml=winrt::Windows::UI::Xaml;
namespace controls=xaml::Controls;
namespace hosting=xaml::Hosting;
namespace json=winrt::Windows::Data::Json;
constexpr UINT FrameMessage=WM_APP+77;
constexpr UINT InputMessage=WM_APP+78;

// Windows.UI.Xaml's island has an opaque emergency backing visual even when
// SwapChainPanel pixels carry alpha. This private, queried interface is confined
// to our standalone diagnostic process; never apply it to Explorer's XAML Window.
// Only the known property prefix is declared. Unsupported systems fail this
// optional host explicitly instead of rendering a misleading black rectangle.
MIDL_INTERFACE("06636C29-5A17-458D-8EA2-2422D997A922") PeripheralXamlWindowTransparency : ::IInspectable {
    virtual HRESULT STDMETHODCALLTYPE get_TransparentBackground(boolean*)=0;
    virtual HRESULT STDMETHODCALLTYPE put_TransparentBackground(boolean)=0;
};

struct Host {
    std::filesystem::path dir;
    HWND hwnd{},island{},producer{};
    uint64_t generation{},frames{},clicks{};
    uint32_t pixel_width{},pixel_height{};
    bool embedded=false;
    std::string error;
    hosting::WindowsXamlManager manager{nullptr};
    hosting::DesktopWindowXamlSource source{nullptr};
    controls::SwapChainPanel panel{nullptr};
    winrt::com_ptr<ID3D11Device> device;
    winrt::com_ptr<ID3D11DeviceContext> context;
    winrt::com_ptr<IDXGISwapChain1> swap;
    std::array<winrt::com_ptr<ID3D11Texture2D>,2> textures;
    std::array<winrt::com_ptr<IDXGIKeyedMutex>,2> mutexes;

    void report() noexcept {
        try {
            json::JsonObject value;
            value.Insert(L"pid",json::JsonValue::CreateNumberValue(GetCurrentProcessId()));
            value.Insert(L"hwnd",json::JsonValue::CreateNumberValue(reinterpret_cast<uintptr_t>(hwnd)));
            value.Insert(L"island",json::JsonValue::CreateNumberValue(reinterpret_cast<uintptr_t>(island)));
            value.Insert(L"visible",json::JsonValue::CreateBooleanValue(IsWindowVisible(hwnd)!=0));
            value.Insert(L"island_visible",json::JsonValue::CreateBooleanValue(IsWindowVisible(island)!=0));
            value.Insert(L"frames",json::JsonValue::CreateNumberValue(static_cast<double>(frames)));
            value.Insert(L"generation",json::JsonValue::CreateNumberValue(static_cast<double>(generation)));
            value.Insert(L"clicks",json::JsonValue::CreateNumberValue(static_cast<double>(clicks)));
            value.Insert(L"backend",json::JsonValue::CreateStringValue(L"xaml_swap_chain_panel"));
            value.Insert(L"native_abi",json::JsonValue::CreateNumberValue(PERIPHERAL_NATIVE_ABI));
            value.Insert(L"native_build",json::JsonValue::CreateStringValue(winrt::to_hstring(PERIPHERAL_NATIVE_BUILD)));
            HMODULE current{};GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS|GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,reinterpret_cast<LPCWSTR>(&FrameMessage),&current);
            wchar_t module_path[32768]{};GetModuleFileNameW(current,module_path,32768);
            value.Insert(L"native_module",json::JsonValue::CreateStringValue(module_path));
            value.Insert(L"error",error.empty()?json::JsonValue::CreateNullValue():json::JsonValue::CreateStringValue(winrt::to_hstring(error)));
            std::ofstream out(dir/L"consumer.json",std::ios::binary|std::ios::trunc);
            out<<winrt::to_string(value.Stringify());
        } catch(...){}
    }
    json::JsonObject metadata() {
        std::ifstream in(dir/L"frame.json",std::ios::binary);std::stringstream bytes;bytes<<in.rdbuf();
        if(bytes.str().size()>16384)throw winrt::hresult_invalid_argument();
        return json::JsonObject::Parse(winrt::to_hstring(bytes.str()));
    }
    void initialize(HWND window) {
        hwnd=window;
        manager=hosting::WindowsXamlManager::InitializeForCurrentThread();
        source=hosting::DesktopWindowXamlSource();
        auto native=source.as<IDesktopWindowXamlSourceNative>();
        winrt::check_hresult(native->AttachToWindow(hwnd));
        winrt::check_hresult(native->get_WindowHandle(&island));
        make_panel();
        source.Content(panel);
        SetWindowPos(island,nullptr,0,0,660,555,SWP_SHOWWINDOW);
        auto xaml_window=xaml::Window::Current();
        if(!xaml_window)throw winrt::hresult_error(E_NOINTERFACE,L"XAML transparency capability unavailable");
        auto transparency=xaml_window.try_as<PeripheralXamlWindowTransparency>();
        if(!transparency)throw winrt::hresult_error(E_NOINTERFACE,L"XAML transparency interface unsupported; use the DirectComposition host");
        winrt::check_hresult(transparency->put_TransparentBackground(true));
        report();
    }
    void make_panel(){
        panel=controls::SwapChainPanel();
        panel.CompositionScaleChanged([this](auto const&,auto const&){try{update_scale();}catch(...) {}});
        panel.PointerPressed([this](auto const&,xaml::Input::PointerRoutedEventArgs const& e){
            auto properties=e.GetCurrentPoint(panel).Properties();
            if(!properties.IsLeftButtonPressed()&&!properties.IsRightButtonPressed())return;
            ++clicks;panel.CapturePointer(e.Pointer());pointer(e,properties.IsRightButtonPressed()?WM_RBUTTONDOWN:WM_LBUTTONDOWN);e.Handled(true);
        });
        panel.PointerReleased([this](auto const&,xaml::Input::PointerRoutedEventArgs const& e){
            auto kind=e.GetCurrentPoint(panel).Properties().PointerUpdateKind();
            if(kind!=winrt::Windows::UI::Input::PointerUpdateKind::LeftButtonReleased&&kind!=winrt::Windows::UI::Input::PointerUpdateKind::RightButtonReleased)return;
            pointer(e,kind==winrt::Windows::UI::Input::PointerUpdateKind::RightButtonReleased?WM_RBUTTONUP:WM_LBUTTONUP);panel.ReleasePointerCapture(e.Pointer());e.Handled(true);
        });
        panel.PointerMoved([this](auto const&,xaml::Input::PointerRoutedEventArgs const& e){pointer(e,WM_MOUSEMOVE);});
        // The plugin owns right-click; do not also open Explorer's taskbar menu.
        panel.RightTapped([](auto const&,xaml::Input::RightTappedRoutedEventArgs const& e){e.Handled(true);});
    }
    void pointer(xaml::Input::PointerRoutedEventArgs const& e,UINT message) {
        if(!producer)return;
        auto point=e.GetCurrentPoint(panel);
        auto position=point.Position();
        auto x=static_cast<short>(position.X*panel.CompositionScaleX());
        auto y=static_cast<short>(position.Y*panel.CompositionScaleY());
        auto flags=(point.Properties().IsLeftButtonPressed()?MK_LBUTTON:0)|(point.Properties().IsRightButtonPressed()?MK_RBUTTON:0);
        PostMessageW(producer,InputMessage,MAKEWPARAM(message,flags),MAKELPARAM(x,y));
    }
    void update_scale(){
        if(!swap||!pixel_width||!pixel_height)return;
        auto sx=panel.CompositionScaleX(),sy=panel.CompositionScaleY();
        if(sx<=0||sy<=0)return;
        DXGI_MATRIX_3X2_F matrix{1.f/sx,0,0,1.f/sy,0,0};
        winrt::check_hresult(swap.as<IDXGISwapChain2>()->SetMatrixTransform(&matrix));
        panel.Width(pixel_width/sx);panel.Height(pixel_height/sy);
    }
    void configure() {
        auto meta=metadata();
        uint32_t width=static_cast<uint32_t>(meta.GetNamedNumber(L"width"));
        uint32_t height=static_cast<uint32_t>(meta.GetNamedNumber(L"height"));
        if(!width||!height||width>4096||height>2160)throw winrt::hresult_invalid_argument();
        if(swap){panel.as<ISwapChainPanelNative>()->SetSwapChain(nullptr);swap=nullptr;}
        winrt::com_ptr<IDXGIFactory2> factory;
        winrt::check_hresult(CreateDXGIFactory1(__uuidof(IDXGIFactory2),factory.put_void()));
        winrt::com_ptr<IDXGIAdapter1> selected;
        for(UINT i=0;i<32;++i){winrt::com_ptr<IDXGIAdapter1> adapter;if(FAILED(factory->EnumAdapters1(i,adapter.put())))break;
            DXGI_ADAPTER_DESC1 d{};adapter->GetDesc1(&d);
            if(d.AdapterLuid.LowPart==static_cast<DWORD>(meta.GetNamedNumber(L"adapter_low")) && d.AdapterLuid.HighPart==static_cast<LONG>(meta.GetNamedNumber(L"adapter_high"))){selected=adapter;break;}}
        if(!selected)throw winrt::hresult_error(E_FAIL,L"GPU adapter missing");
        device=nullptr;context=nullptr;
        winrt::check_hresult(D3D11CreateDevice(selected.get(),D3D_DRIVER_TYPE_UNKNOWN,nullptr,D3D11_CREATE_DEVICE_BGRA_SUPPORT,nullptr,0,D3D11_SDK_VERSION,device.put(),nullptr,context.put()));
        DXGI_SWAP_CHAIN_DESC1 desc{};desc.Width=width;desc.Height=height;desc.Format=DXGI_FORMAT_B8G8R8A8_UNORM;
        desc.SampleDesc.Count=1;desc.BufferUsage=DXGI_USAGE_RENDER_TARGET_OUTPUT;desc.BufferCount=2;
        desc.Scaling=DXGI_SCALING_STRETCH;desc.SwapEffect=DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL;desc.AlphaMode=DXGI_ALPHA_MODE_PREMULTIPLIED;
        winrt::check_hresult(factory->CreateSwapChainForComposition(device.get(),&desc,nullptr,swap.put()));
        winrt::check_hresult(panel.as<ISwapChainPanelNative>()->SetSwapChain(swap.get()));
        pixel_width=width;pixel_height=height;update_scale();
        auto names=meta.GetNamedArray(L"names");if(names.Size()!=2)throw winrt::hresult_invalid_argument();
        auto d1=device.as<ID3D11Device1>();
        for(unsigned i=0;i<2;++i){textures[i]=nullptr;mutexes[i]=nullptr;
            winrt::check_hresult(d1->OpenSharedResourceByName(names.GetStringAt(i).c_str(),DXGI_SHARED_RESOURCE_READ|DXGI_SHARED_RESOURCE_WRITE,__uuidof(ID3D11Texture2D),textures[i].put_void()));
            mutexes[i]=textures[i].as<IDXGIKeyedMutex>();}
        producer=reinterpret_cast<HWND>(static_cast<uintptr_t>(meta.GetNamedNumber(L"producer_hwnd")));
        generation=static_cast<uint64_t>(meta.GetNamedNumber(L"generation"));
        if(!embedded){
            SetWindowPos(hwnd,nullptr,900,250,width,height,SWP_NOACTIVATE|SWP_NOZORDER|SWP_SHOWWINDOW);
            SetWindowPos(island,nullptr,0,0,width,height,SWP_NOACTIVATE|SWP_NOZORDER|SWP_SHOWWINDOW);
        }
        update_scale();
    }
    void frame(unsigned index,uint64_t incoming) noexcept {
        try {
            auto before=generation;
            if(incoming!=generation)configure();
            if(incoming!=generation||index>=2)return;
            // WAIT_TIMEOUT is a positive HRESULT: only S_OK grants ownership.
            if(mutexes[index]->AcquireSync(1,0)!=S_OK)return;
            winrt::com_ptr<ID3D11Texture2D> back;
            HRESULT hr=swap->GetBuffer(0,__uuidof(ID3D11Texture2D),back.put_void());
            if(SUCCEEDED(hr))context->CopyResource(back.get(),textures[index].get());
            auto released=mutexes[index]->ReleaseSync(0);context->Flush();
            winrt::check_hresult(hr);winrt::check_hresult(released);
            winrt::check_hresult(swap->Present(0,0));++frames;
            if(before!=generation)report();
        }catch(winrt::hresult_error const& e){error=winrt::to_string(e.message());report();}
        catch(...){error="native surface failure";report();}
    }
    ~Host(){try{if(panel)panel.as<ISwapChainPanelNative>()->SetSwapChain(nullptr);if(source)source.Close();if(manager)manager.Close();}catch(...) {}}
};

LRESULT CALLBACK WindowProc(HWND hwnd,UINT message,WPARAM w,LPARAM l){
    auto host=reinterpret_cast<Host*>(GetWindowLongPtrW(hwnd,GWLP_USERDATA));
    if(message==WM_NCCREATE){auto create=reinterpret_cast<CREATESTRUCTW*>(l);SetWindowLongPtrW(hwnd,GWLP_USERDATA,reinterpret_cast<LONG_PTR>(create->lpCreateParams));return TRUE;}
    if(host){
        if(message==FrameMessage){host->frame(static_cast<unsigned>(w),l);return 0;}
        if(message==FrameMessage+1){host->report();return 0;}
        if(message==WM_MOUSEACTIVATE)return MA_NOACTIVATE;
        if(message==WM_DESTROY){if(!host->embedded)PostQuitMessage(0);return 0;}
    }
    return DefWindowProcW(hwnd,message,w,l);
}
int run_xaml_host(wchar_t const* directory){
    auto instance=GetModuleHandleW(nullptr);
    auto host=std::make_unique<Host>();host->dir=directory;
    try{
        winrt::init_apartment(winrt::apartment_type::single_threaded);
        WNDCLASSW wc{};wc.lpfnWndProc=WindowProc;wc.hInstance=instance;wc.lpszClassName=L"PeripheralXamlSurface";wc.hCursor=LoadCursorW(nullptr,IDC_ARROW);RegisterClassW(&wc);
        // The XAML island supplies the premultiplied-alpha composition visual.
        // A redirected GDI backing bitmap would paint black under transparent pixels.
        auto hwnd=CreateWindowExW(WS_EX_NOACTIVATE|WS_EX_TOOLWINDOW|WS_EX_NOREDIRECTIONBITMAP,wc.lpszClassName,L"Peripheral · XAML texture host",WS_POPUP,900,250,660,555,nullptr,nullptr,instance,host.get());
        if(!hwnd)winrt::throw_last_error();host->initialize(hwnd);ShowWindow(hwnd,SW_SHOWNOACTIVATE);
        MSG msg{};while(GetMessageW(&msg,nullptr,0,0)>0){TranslateMessage(&msg);DispatchMessageW(&msg);}
        return 0;
    }catch(winrt::hresult_error const&e){host->error=winrt::to_string(e.message());host->report();return 1;}
    catch(...){host->error="XAML host initialization failed";host->report();return 1;}
}
#ifdef PERIPHERAL_LIBRARY
extern "C" __declspec(dllexport) int __stdcall PeripheralRunXamlHost(wchar_t const* directory){if(!directory)return 2;return run_xaml_host(directory);}
#else
int WINAPI wWinMain(HINSTANCE,HINSTANCE,PWSTR,int){
    int count{};auto args=CommandLineToArgvW(GetCommandLineW(),&count);
    if(count!=2){LocalFree(args);return 2;}
    std::wstring directory=args[1];LocalFree(args);return run_xaml_host(directory.c_str());
}
#endif
