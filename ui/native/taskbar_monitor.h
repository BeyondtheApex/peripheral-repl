#pragma once
#include <windows.h>
#include <cwchar>

namespace peripheral::taskbar {
// XAML AutomationPeer bounds can be root-local for desktop islands. Identify
// ownership through native taskbar/ContentBridge HWNDs instead. Each candidate
// is checked on its XAML dispatcher thread. Ambiguous shared threads fail closed.
struct TaskbarWindow {HWND bar{},island{};};
inline HWND taskbar_ancestor(HWND island) noexcept {
    for(auto node=island;node;node=GetParent(node)){
        DWORD pid{};GetWindowThreadProcessId(node,&pid);
        if(pid!=GetCurrentProcessId())return nullptr;
        wchar_t type[64]{};GetClassNameW(node,type,64);
        if(!wcscmp(type,L"Shell_TrayWnd")||!wcscmp(type,L"Shell_SecondaryTrayWnd"))return node;
    }
    return nullptr;
}
inline TaskbarWindow taskbar_for_thread(DWORD thread=GetCurrentThreadId()) noexcept {
    struct Context {DWORD thread;TaskbarWindow target{};size_t matches{};} context{thread};
    EnumWindows([](HWND bar,LPARAM value)->BOOL {
        auto& context=*reinterpret_cast<Context*>(value);
        DWORD pid{};GetWindowThreadProcessId(bar,&pid);
        if(pid!=GetCurrentProcessId())return TRUE;
        wchar_t type[64]{};GetClassNameW(bar,type,64);
        if(wcscmp(type,L"Shell_TrayWnd")&&wcscmp(type,L"Shell_SecondaryTrayWnd"))return TRUE;
        struct Children {Context& context;HWND bar;} children{context,bar};
        EnumChildWindows(bar,[](HWND island,LPARAM value)->BOOL {
            auto& children=*reinterpret_cast<Children*>(value);
            if(GetWindowThreadProcessId(island,nullptr)!=children.context.thread)return TRUE;
            wchar_t type[96]{};GetClassNameW(island,type,96);
            if(wcscmp(type,L"Windows.UI.Composition.DesktopWindowContentBridge"))return TRUE;
            ++children.context.matches;children.context.target={children.bar,island};
            return TRUE;
        },reinterpret_cast<LPARAM>(&children));
        return TRUE;
    },reinterpret_cast<LPARAM>(&context));
    return context.matches==1?context.target:TaskbarWindow{};
}
inline bool primary_window(HWND bar) noexcept {
    if(!bar||!IsWindow(bar))return false;
    wchar_t type[64]{};GetClassNameW(bar,type,64);
    if(wcscmp(type,L"Shell_TrayWnd"))return false;
    MONITORINFO info{sizeof(info)};
    auto monitor=MonitorFromWindow(bar,MONITOR_DEFAULTTONULL);
    return monitor&&GetMonitorInfoW(monitor,&info)&&(info.dwFlags&MONITORINFOF_PRIMARY);
}
}
