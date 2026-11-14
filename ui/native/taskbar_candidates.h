#pragma once
#include <map>
#include <mutex>
#include <atomic>
#include <memory>

// Discovery can arrive on different XAML threads and in any monitor order.
// Resolve weak XAML references and run attachment only on their own dispatcher.
class TaskbarCandidates {
    struct Entry {
        std::atomic<bool> alive{true};
        winrt::weak_ref<xaml::FrameworkElement> element;
        xaml::DispatcherTimer timer{nullptr};
    };
    std::mutex mutex;
    std::map<InstanceHandle,std::shared_ptr<Entry>> entries;
public:
    void remove(InstanceHandle id){
        std::lock_guard lock(mutex);
        auto it=entries.find(id);if(it==entries.end())return;
        it->second->alive=false;entries.erase(it);
    }
    void add(InstanceHandle id,xaml::FrameworkElement const& element,
             std::function<void(xaml::FrameworkElement const&)> attempt,
             std::function<void(char const*,HRESULT)> report){
        auto entry=std::make_shared<Entry>();entry->element=winrt::make_weak(element);
        {
            std::lock_guard lock(mutex);
            std::erase_if(entries,[](auto const& item){return !item.second->alive;});
            if(entries.contains(id))return;
            if(entries.size()>=64){report("candidate_limit",E_BOUNDS);return;}
            entries.emplace(id,entry);
        }
        try{
            auto operation=element.Dispatcher().RunAsync(winrt::Windows::UI::Core::CoreDispatcherPriority::Normal,[entry,attempt,report]{
                try{
                if(!entry->alive)return;
                report("candidate_dispatch_enter",S_OK);
                entry->timer=xaml::DispatcherTimer();entry->timer.Interval(std::chrono::milliseconds(250));
                // The timer holds entry until removal/expiration, then breaks the cycle on its UI thread.
                entry->timer.Tick([entry,attempt,report](auto const&,auto const&){
                    try{
                        auto element=entry->alive?entry->element.get():nullptr;
                        if(!element){entry->alive=false;entry->timer.Stop();entry->timer=nullptr;report("candidate_expired",S_OK);return;}
                        attempt(element);
                    }catch(...){report("candidate_tick_failed",winrt::to_hresult());entry->alive=false;}
                });
                entry->timer.Start();
                if(auto element=entry->element.get())attempt(element);
                }catch(...){entry->alive=false;report("candidate_initialize_failed",winrt::to_hresult());}
            });
            operation.Completed([report](auto const& operation,auto status){
                if(status==winrt::Windows::Foundation::AsyncStatus::Error)report("candidate_dispatch_failed",operation.ErrorCode());
                else if(status==winrt::Windows::Foundation::AsyncStatus::Canceled)report("candidate_dispatch_cancelled",E_ABORT);
            });
        }catch(...){report("candidate_queue_failed",winrt::to_hresult());remove(id);}
    }
};
