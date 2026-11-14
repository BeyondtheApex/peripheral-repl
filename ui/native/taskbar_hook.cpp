// Optional private Windows taskbar adapter. GPUI, Lua and device I/O stay out of Explorer.
#define PERIPHERAL_TASKBAR
#include "xaml_host.cpp"
#include <winrt/Windows.UI.Core.h>
#include <atomic>
#include <mutex>
#include "taskbar_trace.h"
#include "taskbar_placement.h"
#include "taskbar_monitor.h"
#include "taskbar_candidates.h"
#include "taskbar_roots.h"

namespace core=winrt::Windows::UI::Core;
constexpr GUID TapId={0x479be21b,0x8374,0x4cec,{0x8b,0x63,0xa8,0x0b,0x16,0x3d,0x29,0x92}};
HMODULE module_handle{};
std::atomic<bool> attached{false};
std::atomic<bool> stopped{false};
std::unique_ptr<Host> bridge;
std::unique_ptr<TaskbarPlacement> placement;
xaml::DispatcherTimer watchdog{nullptr};
std::filesystem::path session_path;
HANDLE producer_process{};
HANDLE stop_event{};
TaskbarCandidates candidates;
TaskbarRoots taskbar_roots;
std::mutex owner_mutex;
core::CoreDispatcher owner_dispatcher{nullptr};
uint64_t requested_generation{};
HWND control_window{};
std::mutex trace_mutex;
std::filesystem::path trace_directory;
void trace(char const* stage,HRESULT hr=S_OK) noexcept {try{std::lock_guard lock(trace_mutex);if(!trace_directory.empty())taskbar_trace(trace_directory,stage,hr);}catch(...){}}
std::filesystem::path current_config(){std::lock_guard lock(owner_mutex);return session_path;}
void select_config(std::filesystem::path const& config){
    {std::lock_guard lock(owner_mutex);session_path=config;}
    std::lock_guard lock(trace_mutex);trace_directory=config.parent_path();
}
json::JsonObject read_config(std::filesystem::path const& path){
    if(std::filesystem::file_size(path)>16384)throw winrt::hresult_invalid_argument();
    std::ifstream in(path,std::ios::binary);std::stringstream data;data<<in.rdbuf();
    auto config=json::JsonObject::Parse(winrt::to_hstring(data.str()));
    if(config.GetNamedNumber(L"protocol")!=PERIPHERAL_NATIVE_ABI||config.GetNamedString(L"native_build")!=winrt::to_hstring(PERIPHERAL_NATIVE_BUILD))
        throw winrt::hresult_error(E_FAIL,L"native protocol/build mismatch; restart Explorer normally after updating");
    wchar_t module[32768]{};GetModuleFileNameW(module_handle,module,32768);
    if(_wcsicmp(config.GetNamedString(L"native_dll").c_str(),module))
        throw winrt::hresult_error(E_FAIL,L"another installation is loaded; restart Explorer normally before switching");
    if(std::filesystem::path(config.GetNamedString(L"directory").c_str())!=path.parent_path())throw winrt::hresult_invalid_argument();
    return config;
}
void bootstrap_config(){
    auto name=L"Local\\PeripheralTaskbarConfig_v2_"+std::to_wstring(GetCurrentProcessId());
    auto mapping=OpenFileMappingW(FILE_MAP_READ,FALSE,name.c_str());if(!mapping)winrt::throw_last_error();
    auto view=static_cast<wchar_t const*>(MapViewOfFile(mapping,FILE_MAP_READ,0,0,65536));
    if(!view){CloseHandle(mapping);winrt::throw_last_error();}
    auto length=wcsnlen_s(view,32768);std::wstring path;
    if(length<32768)path.assign(view,length);
    UnmapViewOfFile(view);CloseHandle(mapping);
    if(path.empty())throw winrt::hresult_invalid_argument();
    auto config_path=std::filesystem::path(path);read_config(config_path);select_config(config_path);
}
LRESULT CALLBACK ControlProc(HWND,UINT,WPARAM,LPARAM);

void detach() noexcept {
    trace("detach");
    stopped=true;
    try{
        if(watchdog){watchdog.Stop();watchdog=nullptr;}
        placement.reset();
        if(bridge){if(bridge->hwnd)DestroyWindow(bridge->hwnd);bridge.reset();}
        if(producer_process){CloseHandle(producer_process);producer_process=nullptr;}
        if(stop_event){CloseHandle(stop_event);stop_event=nullptr;}
    }catch(...){}
    attached=false;
}

HWND primary_taskbar(xaml::FrameworkElement const& anchor) noexcept {
    try {
        if(!anchor.Dispatcher().HasThreadAccess()||!anchor.XamlRoot())return nullptr;
        auto target=taskbar_roots.match(anchor);
        return peripheral::taskbar::primary_window(target.bar)?target.bar:nullptr;
    }catch(...){return nullptr;}
}

bool anchor_ready(xaml::FrameworkElement const& tray) noexcept {
    try {
        auto node=tray.Parent();
        for(int i=0;node&&i<24;++i){
            if(auto grid=node.try_as<controls::Grid>();grid&&grid.ActualWidth()>tray.ActualWidth()
                &&grid.ActualHeight()>0&&grid.ActualHeight()<=160)return true;
            node=xaml::Media::VisualTreeHelper::GetParent(node);
        }
    }catch(...){}
    return false;
}

void attach(xaml::FrameworkElement const& tray) noexcept {
    if(stopped||attached)return;
    auto target=primary_taskbar(tray);
    // Diagnostics are rate-limited per candidate thread, not on every timer tick.
    static thread_local std::filesystem::path checked_config;
    static thread_local int last_selection=-1;
    auto config_path=current_config();
    auto native=taskbar_roots.match(tray);
    int selection=!native.bar?0:!target?1:!anchor_ready(tray)?2:3;
    if(config_path!=checked_config||selection!=last_selection){
        checked_config=config_path;last_selection=selection;
        trace(native.exact?"target_resolved_from_xaml_source":"target_thread_fallback");
        trace(selection==0?"primary_target_ambiguous_or_missing":selection==1?"secondary_target_skipped":selection==2?"primary_layout_not_ready":"primary_target_ready");
    }
    // An early visual-tree callback can precede measure/arrange. Wait for a later
    // candidate tick instead of marking the entire adapter stopped permanently.
    if(selection!=3)return;
    try{
        {
            std::lock_guard lock(owner_mutex);
            if(stopped||attached)return;
            attached=true;
            owner_dispatcher=tray.Dispatcher();
        }
        trace("primary_anchor_selected");
        if(!control_window){
            WNDCLASSW wc{};wc.lpfnWndProc=ControlProc;wc.hInstance=module_handle;wc.lpszClassName=L"PeripheralTaskbarControl_v2";RegisterClassW(&wc);
            control_window=CreateWindowExW(0,wc.lpszClassName,L"",0,0,0,0,0,HWND_MESSAGE,nullptr,module_handle,nullptr);
            if(!control_window)winrt::throw_last_error();
        }
        auto config=read_config(current_config());
        if(config.GetNamedNumber(L"explorer_pid")!=GetCurrentProcessId())throw winrt::hresult_error(E_FAIL,L"Explorer generation changed");
        auto pid=static_cast<DWORD>(config.GetNamedNumber(L"producer_pid"));
        producer_process=OpenProcess(SYNCHRONIZE,FALSE,pid);
        if(!producer_process||WaitForSingleObject(producer_process,0)!=WAIT_TIMEOUT)throw winrt::hresult_error(E_FAIL,L"producer stopped");
        stop_event=OpenEventW(SYNCHRONIZE,FALSE,config.GetNamedString(L"stop_event").c_str());
        if(!stop_event)winrt::throw_last_error();
        if(WaitForSingleObject(stop_event,0)==WAIT_OBJECT_0)throw winrt::hresult_error(E_ABORT,L"attachment cancelled");
        bridge=std::make_unique<Host>();bridge->dir=config.GetNamedString(L"directory").c_str();bridge->embedded=true;
        bridge->make_panel();bridge->panel.Name(L"PeripheralTaskbarSurface");bridge->panel.Width(220);bridge->panel.Height(40);
        bridge->panel.VerticalAlignment(xaml::VerticalAlignment::Center);
        WNDCLASSW wc{};wc.lpfnWndProc=WindowProc;wc.hInstance=module_handle;wc.lpszClassName=L"PeripheralTaskbarBridge_v2";RegisterClassW(&wc);
        bridge->hwnd=CreateWindowExW(0,wc.lpszClassName,L"",0,0,0,0,0,HWND_MESSAGE,nullptr,module_handle,bridge.get());
        if(!bridge->hwnd)winrt::throw_last_error();
        auto anchor=winrt::make_weak(tray);
        placement=std::make_unique<TaskbarPlacement>(*bridge,tray,config.GetNamedString(L"side",L"right")==L"left",TaskbarPlacement::centered,
            [anchor,target]{auto tray=anchor.get();return tray&&primary_taskbar(tray)==target;});
        bridge->report();trace("panel_attached");
        watchdog=xaml::DispatcherTimer();watchdog.Interval(std::chrono::milliseconds(250));
        watchdog.Tick([](auto const&,auto const&){
            if(WaitForSingleObject(producer_process,0)!=WAIT_TIMEOUT||WaitForSingleObject(stop_event,0)==WAIT_OBJECT_0){detach();return;}
            if(placement)placement->refresh();
        });watchdog.Start();
    }catch(winrt::hresult_error const& e){trace("attach_error",e.code());if(bridge){bridge->error=winrt::to_string(e.message());bridge->report();}detach();}
    catch(...){trace("attach_exception",E_FAIL);detach();}
}

struct Watcher:winrt::implements<Watcher,IVisualTreeServiceCallback2,winrt::non_agile>{
    winrt::com_ptr<IXamlDiagnostics> diagnostics;
    explicit Watcher(winrt::com_ptr<IUnknown> const& site):diagnostics(site.as<IXamlDiagnostics>()){}
    HRESULT STDMETHODCALLTYPE OnVisualTreeChange(ParentChildRelation relation,VisualElement element,VisualMutationType mutation) noexcept override{
        static std::atomic<bool> first{true};if(first.exchange(false))trace("tree_callback");
        if(mutation==Remove){candidates.remove(element.Handle);taskbar_roots.remove(element.Handle);return S_OK;}
        if(mutation==Add&&element.Type&&(!relation.Parent||wcsstr(element.Type,L"DesktopWindowXamlSource"))){
            try{
                winrt::Windows::Foundation::IInspectable object;
                winrt::check_hresult(diagnostics->GetIInspectableFromHandle(element.Handle,reinterpret_cast<::IInspectable**>(winrt::put_abi(object))));
                if(auto source=object.try_as<hosting::DesktopWindowXamlSource>()){
                    taskbar_roots.add(element.Handle,source);trace("xaml_source_registered");
                }
            }catch(...){trace("xaml_source_resolution_failed",winrt::to_hresult());}
        }
        if(mutation!=Add||!element.Type||wcscmp(element.Type,L"SystemTray.SystemTrayFrame"))return S_OK;
        trace("tray_anchor_found");
        try{winrt::Windows::Foundation::IInspectable object;
            winrt::check_hresult(diagnostics->GetIInspectableFromHandle(element.Handle,reinterpret_cast<::IInspectable**>(winrt::put_abi(object))));
            if(auto tray=object.try_as<xaml::FrameworkElement>())candidates.add(element.Handle,tray,attach,trace);
            else trace("tray_anchor_type_unavailable",E_NOINTERFACE);
        }catch(...){trace("tray_anchor_resolution_failed",winrt::to_hresult());}return S_OK;
    }
    HRESULT STDMETHODCALLTYPE OnElementStateChanged(InstanceHandle,VisualElementState,LPCWSTR) noexcept override{return S_OK;}
};
winrt::com_ptr<Watcher> watcher;
struct Tap:winrt::implements<Tap,IObjectWithSite,winrt::non_agile>{
    winrt::com_ptr<IUnknown> site;
    HRESULT STDMETHODCALLTYPE SetSite(IUnknown* value) noexcept override{
        trace("set_site");
        try{if(current_config().empty())bootstrap_config();
            if(watcher){winrt::check_hresult(watcher->diagnostics.as<IVisualTreeService3>()->UnadviseVisualTreeChange(watcher.get()));watcher=nullptr;}
            site.copy_from(value);if(site){
            watcher=winrt::make_self<Watcher>(site);
            auto retained=watcher.get();retained->AddRef();
            auto worker=CreateThread(nullptr,0,[](void* value)->DWORD{
                winrt::com_ptr<Watcher> current;current.attach(static_cast<Watcher*>(value));
                try{winrt::init_apartment(winrt::apartment_type::multi_threaded);trace("advise_enter");auto hr=current->diagnostics.as<IVisualTreeService3>()->AdviseVisualTreeChange(current.get());trace("advise_return",hr);}
                catch(...){trace("advise_exception",winrt::to_hresult());}return 0;
            },retained,0,nullptr);
            if(worker)CloseHandle(worker);else{retained->Release();winrt::throw_last_error();}}
            return S_OK;
        }catch(...){auto hr=winrt::to_hresult();trace("set_site_error",hr);return hr;}
    }
    HRESULT STDMETHODCALLTYPE GetSite(REFIID iid,void** out) noexcept override{if(!out)return E_POINTER;*out=nullptr;if(site)return site.as(iid,out);return E_FAIL;}
};
struct Factory:winrt::implements<Factory,IClassFactory>{
    HRESULT STDMETHODCALLTYPE CreateInstance(IUnknown* outer,REFIID iid,void** out) noexcept override{if(outer)return CLASS_E_NOAGGREGATION;try{return winrt::make<Tap>().as(iid,out);}catch(...){return winrt::to_hresult();}}
    HRESULT STDMETHODCALLTYPE LockServer(BOOL) noexcept override{return S_OK;}
};
extern "C" __declspec(dllexport) HRESULT __stdcall DllGetClassObject(REFCLSID clsid,REFIID iid,void** out){trace("get_class_object");if(clsid!=TapId)return CLASS_E_CLASSNOTAVAILABLE;try{return winrt::make<Factory>().as(iid,out);}catch(...){return winrt::to_hresult();}}
extern "C" __declspec(dllexport) HRESULT __stdcall DllCanUnloadNow(){return S_FALSE;}
void schedule_reattach(std::filesystem::path config,uint64_t generation){
    core::CoreDispatcher dispatcher{nullptr};
    {std::lock_guard lock(owner_mutex);dispatcher=owner_dispatcher;}
    if(!dispatcher)throw winrt::hresult_error(E_FAIL,L"taskbar dispatcher unavailable");
    dispatcher.RunAsync(core::CoreDispatcherPriority::Normal,[config,generation]{
        try{
            bool moved=false;
            {
                std::lock_guard lock(owner_mutex);
                if(generation!=requested_generation)return;
                moved=!owner_dispatcher.HasThreadAccess();
                if(!moved)stopped=true; // Prevent another thread claiming ownership during teardown.
            }
            if(moved){schedule_reattach(config,generation);return;}
            detach();select_config(config);stopped=false;trace("reattach_dispatch");
        }catch(...){trace("reattach_dispatch_error",winrt::to_hresult());}
    });
}
LRESULT CALLBACK ControlProc(HWND hwnd,UINT message,WPARAM w,LPARAM l){
    if(message==WM_COPYDATA){
        try{
            auto data=reinterpret_cast<COPYDATASTRUCT const*>(l);
            if(!data||data->dwData!=0x5052504c||!data->lpData||data->cbData<sizeof(wchar_t)||data->cbData>65536||data->cbData%sizeof(wchar_t))return 0;
            auto text=static_cast<wchar_t const*>(data->lpData);auto count=data->cbData/sizeof(wchar_t);
            if(text[count-1]!=L'\0')return 0;
            std::filesystem::path config(std::wstring(text,count-1));
            // Only another generation in this installation's bridge root.
            if(config.filename()!=L"taskbar.json"||!config.is_absolute()||config.lexically_normal()!=config)return 0;
            if(config.parent_path().parent_path()!=current_config().parent_path().parent_path())return 0;
            read_config(config);
            uint64_t generation;
            {std::lock_guard lock(owner_mutex);generation=++requested_generation;}
            schedule_reattach(config,generation);return 1;
        }catch(...){trace("reattach_error",winrt::to_hresult());}return 0;
    }
    return DefWindowProcW(hwnd,message,w,l);
}
BOOL WINAPI DllMain(HINSTANCE instance,DWORD reason,void*){
    // No COM, file I/O or worker creation under the loader lock.
    if(reason==DLL_PROCESS_ATTACH)module_handle=instance;
    return TRUE;
}
