#include <windows.h>
#include <filesystem>
#include <iostream>
#include <string>
#include <fstream>
#include <xamlom.h>
#include <objbase.h>
#include "taskbar_trace.h"
#include "native_build.h"

namespace {
constexpr ULONG_PTR ReattachMessage=0x5052504c;
struct Handle {HANDLE value{INVALID_HANDLE_VALUE};~Handle(){if(value!=INVALID_HANDLE_VALUE&&value)CloseHandle(value);}};
std::filesystem::path physical_path(wchar_t const* path){
    Handle file{CreateFileW(path,FILE_READ_ATTRIBUTES,FILE_SHARE_READ|FILE_SHARE_WRITE|FILE_SHARE_DELETE,nullptr,OPEN_EXISTING,FILE_FLAG_BACKUP_SEMANTICS,nullptr)};
    if(file.value==INVALID_HANDLE_VALUE)throw std::system_error(GetLastError(),std::system_category(),"open path");
    auto length=GetFinalPathNameByHandleW(file.value,nullptr,0,FILE_NAME_NORMALIZED|VOLUME_NAME_DOS);
    if(!length)throw std::system_error(GetLastError(),std::system_category(),"resolve physical path");
    std::wstring result(length,L'\0');auto written=GetFinalPathNameByHandleW(file.value,result.data(),length,FILE_NAME_NORMALIZED|VOLUME_NAME_DOS);
    if(!written||written>=length)throw std::system_error(GetLastError(),std::system_category(),"resolve physical path");
    result.resize(written);return result;
}
bool wait_for_host(std::filesystem::path const& directory){
    // Consumer report is the acknowledgement; InitializeXamlDiagnosticsEx S_OK isn't.
    for(int i=0;i<100;++i){
        std::error_code error;auto bytes=std::filesystem::file_size(directory/L"consumer.json",error);
        if(!error&&bytes>2){taskbar_trace(directory,"consumer_acknowledged");return true;}
        Sleep(100);
    }
    taskbar_trace(directory,"acknowledgement_timeout",HRESULT_FROM_WIN32(WAIT_TIMEOUT));return false;
}
}
int run_taskbar_loader(int argc,wchar_t**argv){
    try {
        if(argc!=3&&argc!=4)return 2;
        auto dll=physical_path(argv[2]);
        if(!wcscmp(argv[1],L"--resolve")){
            // Read-only preflight: no LoadLibrary, hooks, or Explorer communication.
            std::wcout<<dll.native()<<L"\n";return 0;
        }
        if(!wcscmp(argv[1],L"--check")){
            auto module=LoadLibraryW(dll.c_str());auto error=GetLastError();
            std::wcout<<L"local_load="<<(module!=nullptr)<<L" error="<<(module?0:error)<<L"\n";
            if(module)FreeLibrary(module);return module?0:9;
        }
        if(argc!=4)return 2;
        auto config_path=physical_path(argv[3]);auto directory=config_path.parent_path();
        taskbar_trace(directory,"loader_started");
        using GetVersion=LONG(WINAPI*)(OSVERSIONINFOW*);
        auto get_version=reinterpret_cast<GetVersion>(GetProcAddress(GetModuleHandleW(L"ntdll.dll"),"RtlGetVersion"));
        OSVERSIONINFOW version{};version.dwOSVersionInfoSize=sizeof(version);
        if(!get_version||get_version(&version)<0||version.dwMajorVersion!=10||version.dwBuildNumber<22000){taskbar_trace(directory,"unsupported_windows_taskbar",E_NOTIMPL);return 17;}
        wchar_t* end{};auto number=wcstoul(argv[1],&end,10);if(!number||*end)return 2;DWORD pid=number;
        HWND taskbar=FindWindowW(L"Shell_TrayWnd",nullptr);DWORD actual{};GetWindowThreadProcessId(taskbar,&actual);
        if(!taskbar||actual!=pid){taskbar_trace(directory,"taskbar_identity_mismatch",E_INVALIDARG);return 4;}
        Handle process{OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,FALSE,pid)};
        if(!process.value){taskbar_trace(directory,"process_open_error",HRESULT_FROM_WIN32(GetLastError()));return 3;}
        USHORT machine{},native_machine{};
        if(!IsWow64Process2(process.value,&machine,&native_machine)||machine!=IMAGE_FILE_MACHINE_UNKNOWN||native_machine!=IMAGE_FILE_MACHINE_AMD64){taskbar_trace(directory,"unsupported_explorer_architecture",E_NOTIMPL);return 18;}
        wchar_t image[MAX_PATH]{};DWORD length=MAX_PATH;
        if(!QueryFullProcessImageNameW(process.value,0,image,&length)||_wcsicmp(std::filesystem::path(image).filename().c_str(),L"explorer.exe"))return 4;
        taskbar_trace(directory,"physical_path_resolved");
        HWND control=nullptr;
        while((control=FindWindowExW(HWND_MESSAGE,control,L"PeripheralTaskbarControl_v2",nullptr))){
            DWORD owner{};GetWindowThreadProcessId(control,&owner);if(owner!=pid)continue;
            auto config=config_path.wstring();
            COPYDATASTRUCT data{ReattachMessage,static_cast<DWORD>((config.size()+1)*sizeof(wchar_t)),config.data()};DWORD_PTR result{};
            bool sent=SendMessageTimeoutW(control,WM_COPYDATA,0,reinterpret_cast<LPARAM>(&data),SMTO_ABORTIFHUNG|SMTO_BLOCK,2000,&result)!=0;
            taskbar_trace(directory,"reattach_requested",sent&&result?S_OK:E_FAIL);
            if(!sent||!result){taskbar_trace(directory,"loaded_module_incompatible_restart_required",E_FAIL);return 8;}
            return wait_for_host(directory)?0:14;
        }
        // Bootstrap data is separate from the immutable installation directory.
        // Explorer may not share the launcher's filesystem virtualization.
        auto config=config_path.wstring();if(config.size()>=32768)return 2;
        auto mapping_name=L"Local\\PeripheralTaskbarConfig_v2_"+std::to_wstring(pid);
        Handle mapping{CreateFileMappingW(INVALID_HANDLE_VALUE,nullptr,PAGE_READWRITE,0,65536,mapping_name.c_str())};
        if(!mapping.value)return 15;
        if(GetLastError()==ERROR_ALREADY_EXISTS)return 16;
        auto view=MapViewOfFile(mapping.value,FILE_MAP_WRITE,0,0,65536);if(!view)return 15;
        memcpy(view,config.c_str(),(config.size()+1)*sizeof(wchar_t));UnmapViewOfFile(view);
        auto com=CoInitializeEx(nullptr,COINIT_MULTITHREADED);if(FAILED(com))return 10;
        auto xaml=LoadLibraryExW(L"Windows.UI.Xaml.dll",nullptr,LOAD_LIBRARY_SEARCH_SYSTEM32);
        if(!xaml){taskbar_trace(directory,"xaml_load_error",HRESULT_FROM_WIN32(GetLastError()));return 10;}
        auto diagnostics=reinterpret_cast<decltype(&InitializeXamlDiagnosticsEx)>(GetProcAddress(xaml,"InitializeXamlDiagnosticsEx"));
        if(!diagnostics)return 11;
        constexpr GUID tap={0x479be21b,0x8374,0x4cec,{0x8b,0x63,0xa8,0x0b,0x16,0x3d,0x29,0x92}};
        for(int i=1;i<=64;++i){auto endpoint=L"VisualDiagConnection"+std::to_wstring(i);
            auto hr=diagnostics(endpoint.c_str(),pid,L"",dll.c_str(),tap,nullptr);
            if(hr==HRESULT_FROM_WIN32(ERROR_NOT_FOUND))continue;
            taskbar_trace(directory,"diagnostics_return",hr);
            if(FAILED(hr))return 12;
            return wait_for_host(directory)?0:14;
        }
        taskbar_trace(directory,"diagnostics_connection_missing",HRESULT_FROM_WIN32(ERROR_NOT_FOUND));return 13;
    }catch(std::exception const& e){std::cerr<<e.what()<<"\n";return 20;}
}
#ifdef PERIPHERAL_LIBRARY
extern "C" __declspec(dllexport) unsigned __stdcall PeripheralNativeAbi(){return PERIPHERAL_NATIVE_ABI;}
extern "C" __declspec(dllexport) char const* __stdcall PeripheralNativeBuild(){return PERIPHERAL_NATIVE_BUILD;}
extern "C" __declspec(dllexport) int __stdcall PeripheralRunTaskbarLoader(unsigned pid,wchar_t const* dll,wchar_t const* config){
    if(!dll||!config)return 2;
    std::wstring name=L"peripheral-ui",number=std::to_wstring(pid),path=dll,session=config;
    wchar_t* args[]={name.data(),number.data(),path.data(),session.data()};return run_taskbar_loader(4,args);
}
#else
int wmain(int argc,wchar_t**argv){return run_taskbar_loader(argc,argv);}
#endif
