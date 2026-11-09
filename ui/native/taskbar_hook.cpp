// Optional private Windows taskbar adapter. GPUI, Lua and device I/O stay out of Explorer.
#define PERIPHERAL_TASKBAR
#include "xaml_host.cpp"
#include <winrt/Windows.UI.Core.h>
#include <atomic>
#include <mutex>
#include "taskbar_trace.h"

namespace core=winrt::Windows::UI::Core;
constexpr GUID TapId={0x479be21b,0x8374,0x4cec,{0x8b,0x63,0xa8,0x0b,0x16,0x3d,0x29,0x92}};
HMODULE module_handle{};
std::atomic<bool> attached{false};
std::atomic<bool> stopped{false};
std::unique_ptr<Host> bridge;
controls::Grid parent{nullptr};
xaml::DispatcherTimer watchdog{nullptr};
std::filesystem::path session_path;
HANDLE producer_process{};
HANDLE stop_event{};
bool inserted_column=false;
uint32_t column_index{};
winrt::weak_ref<xaml::FrameworkElement> tray_anchor;
HWND control_window{};
std::mutex trace_mutex;
std::filesystem::path trace_directory;
std::filesystem::path module_directory(){wchar_t path[32768]{};GetModuleFileNameW(module_handle,path,32768);return std::filesystem::path(path).parent_path();}
void trace(char const* stage,HRESULT hr=S_OK) noexcept {try{std::lock_guard lock(trace_mutex);if(!trace_directory.empty())taskbar_trace(trace_directory,stage,hr);}catch(...){}}
void select_config(std::filesystem::path const& config){session_path=config;std::lock_guard lock(trace_mutex);trace_directory=config.parent_path();}
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
    stopped=true;attached=false;
    try{
        if(watchdog){watchdog.Stop();watchdog=nullptr;}
        if(parent&&bridge&&bridge->panel){
            uint32_t index{};if(parent.Children().IndexOf(bridge->panel,index))parent.Children().RemoveAt(index);
            if(inserted_column&&column_index<parent.ColumnDefinitions().Size()){
                parent.ColumnDefinitions().RemoveAt(column_index);
                for(auto child:parent.Children()){if(auto element=child.try_as<xaml::FrameworkElement>()){
                    auto col=controls::Grid::GetColumn(element);if(col>static_cast<int>(column_index))controls::Grid::SetColumn(element,col-1);
                }}
            }
        }
        if(bridge){if(bridge->hwnd)DestroyWindow(bridge->hwnd);bridge.reset();}
        parent=nullptr;
        inserted_column=false;
        if(producer_process){CloseHandle(producer_process);producer_process=nullptr;}
        if(stop_event){CloseHandle(stop_event);stop_event=nullptr;}
    }catch(...){}
}

void attach(xaml::FrameworkElement const& tray) noexcept {
    trace("attach_enter");
    if(stopped||attached.exchange(true))return;
    try{
        auto grid=tray.Parent().try_as<controls::Grid>();
        if(!grid)throw winrt::hresult_error(E_FAIL,L"unsupported taskbar parent");
        auto config=read_config(session_path);
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
        parent=grid;
        auto columns=grid.ColumnDefinitions();auto children=grid.Children();
        uint32_t index{};if(!children.IndexOf(tray,index))throw winrt::hresult_error(E_FAIL,L"taskbar anchor missing");
        // Refuse unexpected layouts before changing any Explorer-owned properties.
        for(auto child:children){if(auto element=child.try_as<xaml::FrameworkElement>()){
            if(element.Name()==L"PeripheralTaskbarSurface")throw winrt::hresult_error(E_FAIL,L"taskbar surface already present");
            if(columns.Size()>0&&controls::Grid::GetColumnSpan(element)!=1)throw winrt::hresult_error(E_FAIL,L"unsupported spanning taskbar layout");
        }}
        if(columns.Size()>0){
            int target=controls::Grid::GetColumn(tray);if(target<0||target>static_cast<int>(columns.Size()))throw winrt::hresult_error(E_FAIL,L"unsupported column layout");
            column_index=target; xaml::Controls::ColumnDefinition column;column.Width(xaml::GridLengthHelper::Auto());columns.InsertAt(column_index,column);inserted_column=true;
            for(auto child:children){if(auto element=child.try_as<xaml::FrameworkElement>()){int col=controls::Grid::GetColumn(element);if(col>=target)controls::Grid::SetColumn(element,col+1);}}
            controls::Grid::SetColumn(bridge->panel,target);
        }else{
            auto reserve=tray.ActualWidth();if(reserve<=0||reserve>grid.ActualWidth()-240)throw winrt::hresult_error(E_FAIL,L"taskbar size unavailable");
            bridge->panel.HorizontalAlignment(xaml::HorizontalAlignment::Right);
            bridge->panel.Margin(xaml::ThicknessHelper::FromLengths(0,0,reserve+12,0));
        }
        controls::Grid::SetRow(bridge->panel,controls::Grid::GetRow(tray));
        controls::Canvas::SetZIndex(bridge->panel,10000);
        children.InsertAt(index,bridge->panel);bridge->report();trace("panel_attached");
        watchdog=xaml::DispatcherTimer();watchdog.Interval(std::chrono::seconds(1));
        watchdog.Tick([](auto const&,auto const&){
            if(WaitForSingleObject(producer_process,0)!=WAIT_TIMEOUT||WaitForSingleObject(stop_event,0)==WAIT_OBJECT_0){detach();return;}
            if(!inserted_column&&bridge){if(auto tray=tray_anchor.get()){auto reserve=tray.ActualWidth();if(reserve>0)bridge->panel.Margin(xaml::ThicknessHelper::FromLengths(0,0,reserve+12,0));}}
        });watchdog.Start();
    }catch(winrt::hresult_error const& e){trace("attach_error",e.code());if(bridge){bridge->error=winrt::to_string(e.message());bridge->report();}detach();}
    catch(...){trace("attach_exception",E_FAIL);detach();}
}

struct Watcher:winrt::implements<Watcher,IVisualTreeServiceCallback2,winrt::non_agile>{
    winrt::com_ptr<IXamlDiagnostics> diagnostics;
    explicit Watcher(winrt::com_ptr<IUnknown> const& site):diagnostics(site.as<IXamlDiagnostics>()){}
    HRESULT STDMETHODCALLTYPE OnVisualTreeChange(ParentChildRelation,VisualElement element,VisualMutationType mutation) noexcept override{
        static std::atomic<bool> first{true};if(first.exchange(false))trace("tree_callback");
        if(stopped||attached||mutation!=Add||!element.Type||wcscmp(element.Type,L"SystemTray.SystemTrayFrame"))return S_OK;
        trace("tray_anchor_found");
        try{winrt::Windows::Foundation::IInspectable object;
            winrt::check_hresult(diagnostics->GetIInspectableFromHandle(element.Handle,reinterpret_cast<::IInspectable**>(winrt::put_abi(object))));
            if(auto tray=object.try_as<xaml::FrameworkElement>()){
                tray_anchor=winrt::make_weak(tray);
                // Own the reattach control window on this XAML thread. Do not
                // resolve FrameworkElement weak references from a worker apartment.
                if(!control_window){
                    WNDCLASSW wc{};wc.lpfnWndProc=ControlProc;wc.hInstance=module_handle;wc.lpszClassName=L"PeripheralTaskbarControl_v2";RegisterClassW(&wc);
                    control_window=CreateWindowExW(0,wc.lpszClassName,L"",0,0,0,0,0,HWND_MESSAGE,nullptr,module_handle,nullptr);
                    trace("control_window_ready",control_window?S_OK:HRESULT_FROM_WIN32(GetLastError()));
                }
                tray.Dispatcher().RunAsync(core::CoreDispatcherPriority::Normal,[tray]{attach(tray);});
            }
        }catch(...){}return S_OK;
    }
    HRESULT STDMETHODCALLTYPE OnElementStateChanged(InstanceHandle,VisualElementState,LPCWSTR) noexcept override{return S_OK;}
};
winrt::com_ptr<Watcher> watcher;
struct Tap:winrt::implements<Tap,IObjectWithSite,winrt::non_agile>{
    winrt::com_ptr<IUnknown> site;
    HRESULT STDMETHODCALLTYPE SetSite(IUnknown* value) noexcept override{
        trace("set_site");
        try{if(session_path.empty())bootstrap_config();
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
            if(config.parent_path().parent_path()!=session_path.parent_path().parent_path())return 0;
            read_config(config);
            if(auto tray=tray_anchor.get()){
                tray.Dispatcher().RunAsync(core::CoreDispatcherPriority::Normal,[tray,config]{
                    detach();select_config(config);stopped=false;attached=false;trace("reattach_dispatch");attach(tray);
                });return 1;
            }
            trace("reattach_anchor_missing",E_FAIL);
        }catch(...){trace("reattach_error",winrt::to_hresult());}return 0;
    }
    return DefWindowProcW(hwnd,message,w,l);
}
BOOL WINAPI DllMain(HINSTANCE instance,DWORD reason,void*){
    // No COM, file I/O or worker creation under the loader lock.
    if(reason==DLL_PROCESS_ATTACH)module_handle=instance;
    return TRUE;
}
