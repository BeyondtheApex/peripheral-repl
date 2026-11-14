// Exercise real XAML diagnostics discovery inside this disposable test process.
// The diagnostic DLL is never loaded into Explorer by this test.
#define PERIPHERAL_LIBRARY
#include "xaml_host.cpp"
#include <xamlom.h>
#include <thread>
#include <atomic>
#include <iostream>
int main(){
    try{
        winrt::init_apartment(winrt::apartment_type::single_threaded);
        auto manager=hosting::WindowsXamlManager::InitializeForCurrentThread();
        WNDCLASSW wc{};wc.lpfnWndProc=DefWindowProcW;wc.hInstance=GetModuleHandleW(nullptr);wc.lpszClassName=L"PeripheralDiagnosticsTest";RegisterClassW(&wc);
        std::vector<HWND> windows;
        std::vector<hosting::DesktopWindowXamlSource> sources;
        for(int i=0;i<2;++i){
            auto window=CreateWindowExW(WS_EX_NOACTIVATE|WS_EX_TOOLWINDOW,wc.lpszClassName,L"",WS_POPUP,-20000+i*1500,-20000,1200,48,nullptr,nullptr,wc.hInstance,nullptr);
            windows.push_back(window);
            hosting::DesktopWindowXamlSource source;
            auto native=source.as<IDesktopWindowXamlSourceNative>();winrt::check_hresult(native->AttachToWindow(window));
            HWND island{};winrt::check_hresult(native->get_WindowHandle(&island));
            SetWindowPos(island,nullptr,0,0,1200,48,SWP_NOACTIVATE|SWP_NOZORDER|SWP_SHOWWINDOW);
            controls::Grid root;controls::Button button;root.Children().Append(button);source.Content(root);
            ShowWindow(window,SW_SHOWNOACTIVATE);root.UpdateLayout();sources.push_back(source);
        }
        wchar_t exe[32768]{};GetModuleFileNameW(nullptr,exe,32768);
        auto dll=std::filesystem::path(exe).parent_path()/L"peripheral-taskbar-hook.dll";
        auto dir=std::filesystem::temp_directory_path()/(L"peripheral-discovery-test-"+std::to_wstring(GetCurrentProcessId()));
        std::filesystem::create_directories(dir);
        json::JsonObject config;
        config.Insert(L"protocol",json::JsonValue::CreateNumberValue(PERIPHERAL_NATIVE_ABI));
        config.Insert(L"native_build",json::JsonValue::CreateStringValue(winrt::to_hstring(PERIPHERAL_NATIVE_BUILD)));
        config.Insert(L"native_dll",json::JsonValue::CreateStringValue(dll.native()));
        config.Insert(L"directory",json::JsonValue::CreateStringValue(dir.native()));
        auto path=dir/L"taskbar.json";
        {std::ofstream out(path);out<<winrt::to_string(config.Stringify());}
        auto mapping=CreateFileMappingW(INVALID_HANDLE_VALUE,nullptr,PAGE_READWRITE,0,65536,
            (L"Local\\PeripheralTaskbarConfig_v2_"+std::to_wstring(GetCurrentProcessId())).c_str());
        if(!mapping)winrt::throw_last_error();
        auto view=MapViewOfFile(mapping,FILE_MAP_WRITE,0,0,65536);if(!view)winrt::throw_last_error();
        auto text=path.native();memcpy(view,text.c_str(),(text.size()+1)*sizeof(wchar_t));UnmapViewOfFile(view);
        std::atomic<bool> done=false;HRESULT result=E_FAIL;
        std::thread worker([&]{
            try{
                winrt::init_apartment(winrt::apartment_type::multi_threaded);
                auto module=LoadLibraryExW(L"Windows.UI.Xaml.dll",nullptr,LOAD_LIBRARY_SEARCH_SYSTEM32);
                auto initialize=reinterpret_cast<decltype(&InitializeXamlDiagnosticsEx)>(GetProcAddress(module,"InitializeXamlDiagnosticsEx"));
                if(!initialize)throw winrt::hresult_error(E_NOINTERFACE);
                constexpr GUID tap={0x479be21b,0x8374,0x4cec,{0x8b,0x63,0xa8,0x0b,0x16,0x3d,0x29,0x92}};
                for(int i=1;i<=64;++i){
                    result=initialize((L"VisualDiagConnection"+std::to_wstring(i)).c_str(),GetCurrentProcessId(),L"",dll.c_str(),tap,nullptr);
                    if(result!=HRESULT_FROM_WIN32(ERROR_NOT_FOUND))break;
                }
            }catch(...){result=winrt::to_hresult();}
            done=true;
        });
        size_t registered=0;auto deadline=GetTickCount64()+10000;
        while(GetTickCount64()<deadline){
            MSG msg{};while(PeekMessageW(&msg,nullptr,0,0,PM_REMOVE)){TranslateMessage(&msg);DispatchMessageW(&msg);}
            std::ifstream trace(dir/L"taskbar-trace.jsonl");std::stringstream bytes;bytes<<trace.rdbuf();
            auto log=bytes.str();registered=0;
            for(size_t offset=0;(offset=log.find("xaml_source_registered",offset))!=std::string::npos;offset+=22)++registered;
            if(done&&(FAILED(result)||registered>=2))break;
            Sleep(10);
        }
        if(!done){std::cerr<<"Diagnostic API timed out in test process\n";ExitProcess(1);}
        worker.join();CloseHandle(mapping);
        std::cout<<"diagnostics result="<<std::hex<<result<<" source registrations="<<std::dec<<registered<<" evidence="<<dir<<"\n";
        // Diagnostics retains references until process exit. Process isolation is
        // intentional; don't unload the test adapter while callbacks can run.
        ExitProcess(SUCCEEDED(result)&&registered>=2?0:1);
    }catch(winrt::hresult_error const& e){std::cerr<<winrt::to_string(e.message())<<"\n";return 1;}
}
