#pragma once
#include <windows.h>
#include <filesystem>
#include <sstream>

// Diagnostic records contain stage, process/thread and HRESULT, never device data.
inline void taskbar_trace(std::filesystem::path const& directory,char const* stage,HRESULT hr=S_OK) noexcept {
    try {
        std::ostringstream line;line<<"{\"stage\":\""<<stage<<"\",\"pid\":"<<GetCurrentProcessId()<<",\"tid\":"<<GetCurrentThreadId()<<",\"tick\":"<<GetTickCount64()<<",\"hresult\":"<<static_cast<unsigned long>(hr)<<"}\n";
        auto file=CreateFileW((directory/L"taskbar-trace.jsonl").c_str(),FILE_APPEND_DATA,FILE_SHARE_READ|FILE_SHARE_WRITE|FILE_SHARE_DELETE,nullptr,OPEN_ALWAYS,FILE_ATTRIBUTE_NORMAL,nullptr);
        if(file==INVALID_HANDLE_VALUE)return;auto text=line.str();DWORD written{};WriteFile(file,text.data(),static_cast<DWORD>(text.size()),&written,nullptr);CloseHandle(file);
    }catch(...){}
}
