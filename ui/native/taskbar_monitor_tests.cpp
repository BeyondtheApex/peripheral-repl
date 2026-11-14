// Hidden, process-local fake taskbar HWNDs; never touch Explorer or display settings.
#include "taskbar_monitor.h"
#include <future>
#include <thread>
#include <stdexcept>
#include <iostream>
void require(bool value){if(!value)throw std::runtime_error("taskbar monitor identity regression");}
struct Window {
    HWND hwnd{};
    Window(wchar_t const* name,HWND parent=nullptr){
        hwnd=CreateWindowExW(WS_EX_NOACTIVATE|WS_EX_TOOLWINDOW,name,L"",parent?WS_CHILD:WS_POPUP,
            0,0,100,40,parent,nullptr,GetModuleHandleW(nullptr),nullptr);
        require(hwnd!=nullptr);
    }
    ~Window(){if(hwnd)DestroyWindow(hwnd);}
};
int main(){
    try {
        for(auto name:{L"Shell_TrayWnd",L"Shell_SecondaryTrayWnd",L"Windows.UI.Composition.DesktopWindowContentBridge"}){
            WNDCLASSW wc{};wc.lpfnWndProc=DefWindowProcW;wc.hInstance=GetModuleHandleW(nullptr);wc.lpszClassName=name;
            require(RegisterClassW(&wc)!=0);
        }
        using namespace peripheral::taskbar;
        require(!taskbar_for_thread().bar);
        Window primary(L"Shell_TrayWnd"),island(L"Windows.UI.Composition.DesktopWindowContentBridge",primary.hwnd);
        require(taskbar_for_thread().bar==primary.hwnd&&primary_window(primary.hwnd));
        {
            Window second(L"Shell_SecondaryTrayWnd"),other(L"Windows.UI.Composition.DesktopWindowContentBridge",second.hwnd);
            require(!taskbar_for_thread().bar); // Same-size bars sharing a thread are ambiguous.
            require(!primary_window(second.hwnd)); // Even if the secondary bar overlaps the primary screen.
        }
        require(taskbar_for_thread().bar==primary.hwnd); // Re-evaluate, don't cache ambiguity.
        std::promise<bool> result;
        std::thread worker([&]{
            try {
                Window second(L"Shell_SecondaryTrayWnd"),other(L"Windows.UI.Composition.DesktopWindowContentBridge",second.hwnd);
                result.set_value(taskbar_for_thread().bar==second.hwnd&&!primary_window(second.hwnd));
            }catch(...){result.set_exception(std::current_exception());}
        });
        worker.join();require(result.get_future().get());
        require(taskbar_for_thread().bar==primary.hwnd);
        DestroyWindow(island.hwnd);island.hwnd=nullptr;
        require(!taskbar_for_thread().bar); // Dead island is never reused.
        std::cout<<"Primary/secondary taskbar ownership checks passed\n";
        return 0;
    }catch(std::exception const& error){std::cerr<<error.what()<<"\n";return 1;}
}
