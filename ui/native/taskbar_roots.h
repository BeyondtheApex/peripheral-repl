#pragma once
#include <map>
#include <mutex>
#include <vector>
#include "taskbar_monitor.h"

// Match the actual XamlRoot to its public DesktopWindowXamlSource. Multiple
// same-size islands can share a UI thread; a thread ID is not a root identity.
class TaskbarRoots {
    struct Source { DWORD thread; winrt::weak_ref<hosting::DesktopWindowXamlSource> source; };
    std::mutex mutex;
    std::map<uint64_t,Source> sources;
public:
    void add(uint64_t id,hosting::DesktopWindowXamlSource const& source){
        std::lock_guard lock(mutex);
        if(sources.size()<256||sources.contains(id))sources.insert_or_assign(id,Source{GetCurrentThreadId(),winrt::make_weak(source)});
    }
    void remove(uint64_t id){std::lock_guard lock(mutex);sources.erase(id);}
    struct Match {HWND bar{},island{};bool exact=false;};
    Match match(xaml::FrameworkElement const& anchor){
        auto root=anchor.XamlRoot();if(!root)return {};
        std::vector<std::pair<uint64_t,winrt::weak_ref<hosting::DesktopWindowXamlSource>>> local;
        {
            std::lock_guard lock(mutex);
            for(auto const& [id,entry]:sources)if(entry.thread==GetCurrentThreadId())local.emplace_back(id,entry.source);
        }
        for(auto const& [id,weak]:local){
            auto source=weak.get();if(!source){remove(id);continue;}
            try {
                auto content=source.Content();
                if(!content||content.XamlRoot()!=root)continue;
                auto native=source.try_as<IDesktopWindowXamlSourceNative>();
                HWND island{};
                if(!native||FAILED(native->get_WindowHandle(&island)))return {nullptr,nullptr,true};
                return {peripheral::taskbar::taskbar_ancestor(island),island,true};
            }catch(...){return {nullptr,nullptr,true};}
        }
        // Older/private roots may not expose a DesktopWindowXamlSource. Retain
        // the conservative fallback only when the thread has one native island.
        auto fallback=peripheral::taskbar::taskbar_for_thread();
        return {fallback.bar,fallback.island,false};
    }
};
