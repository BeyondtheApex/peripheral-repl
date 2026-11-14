#pragma once
#include "taskbar_geometry.h"

// Own only our transparent Canvas; never rewrite Explorer's columns or margins.
class TaskbarPlacement {
    controls::Grid root{nullptr};
    controls::Canvas layer{nullptr};
    xaml::FrameworkElement tray{nullptr};
    Host& host;
    winrt::event_token changed{};
    bool updating=false,prefer_left=false;
    std::wstring last_status;
    std::function<bool()> centered_query;
    std::function<bool()> target_valid;
public:
    static bool centered(){
        DWORD alignment=1,size=sizeof(alignment);
        auto result=RegGetValueW(HKEY_CURRENT_USER,L"Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced",
            L"TaskbarAl",RRF_RT_REG_DWORD,nullptr,&alignment,&size);
        return (result==ERROR_SUCCESS&&alignment==1)||result==ERROR_FILE_NOT_FOUND;
    }
    TaskbarPlacement(Host& h,xaml::FrameworkElement const& anchor,bool left,
                     std::function<bool()> alignment=centered,std::function<bool()> target=[] {return true;}):tray(anchor),host(h),prefer_left(left),centered_query(std::move(alignment)),target_valid(std::move(target)){
        auto node=anchor.Parent();
        for(int i=0;node&&i<24;++i){
            if(auto grid=node.try_as<controls::Grid>()){
                if(grid.ActualWidth()>anchor.ActualWidth()&&grid.ActualHeight()>0&&grid.ActualHeight()<=160)root=grid;
            }
            node=xaml::Media::VisualTreeHelper::GetParent(node);
        }
        if(!root)throw winrt::hresult_error(E_FAIL,L"unsupported taskbar root");
        layer=controls::Canvas();layer.Name(L"PeripheralTaskbarSurface");
        controls::Grid::SetColumnSpan(layer,std::max(1u,root.ColumnDefinitions().Size()));
        controls::Grid::SetRowSpan(layer,std::max(1u,root.RowDefinitions().Size()));
        // Null background means empty Canvas space never consumes pointer input.
        try{
            layer.Children().Append(host.panel);
            root.Children().Append(layer);
            host.panel.Visibility(xaml::Visibility::Collapsed);
            host.layout_changed=[this]{refresh();};
            changed=root.LayoutUpdated([this](auto const&,auto const&){refresh();});
            refresh();
        }catch(...){cleanup();throw;}
    }
    ~TaskbarPlacement(){cleanup();}
    void cleanup() noexcept {
        host.layout_changed={};
        try{
            if(root&&changed.value)root.LayoutUpdated(changed);
            if(root&&layer){uint32_t index{};if(root.Children().IndexOf(layer,index))root.Children().RemoveAt(index);}
            if(layer)layer.Children().Clear();
        }catch(...){}
    }
    void refresh() noexcept {
        if(updating)return;
        updating=true;
        try{
            auto geometry=peripheral::taskbar::measure(root,tray,layer);
            auto sx=host.panel.CompositionScaleX(),sy=host.panel.CompositionScaleY();
            double total=0;for(auto w:host.item_widths)total+=w;
            std::vector<double> widths;
            bool primary=target_valid();
            bool valid=primary&&geometry.valid&&std::isfinite(total)&&total>0&&sx>0&&sy>0;
            if(valid){
                // GPUI's offscreen producer may live at a different monitor DPI.
                // Keep layout in the consumer root's DIPs, never source pixels.
                for(auto w:host.item_widths)widths.push_back(w/total*host.logical_width);
            }
            auto centered_now=centered_query();
            valid=valid&&(host.left_order==(prefer_left&&centered_now));
            auto placement=valid?peripheral::taskbar::place(geometry.width,geometry.start_left,geometry.tray_left,
                prefer_left,centered_now,geometry.occupied,widths):peripheral::taskbar::Placement{};
            double height=host.logical_height;
            if(height>geometry.height||height<=0)placement.visible=0;
            bool visible=placement.visible>0;
            json::JsonObject status;
            status.Insert(L"primary_monitor",json::JsonValue::CreateBooleanValue(primary));
            status.Insert(L"requested_side",json::JsonValue::CreateStringValue(prefer_left?L"left":L"right"));
            status.Insert(L"effective_side",json::JsonValue::CreateStringValue(prefer_left&&centered_now?L"left":L"right"));
            status.Insert(L"visible_items",json::JsonValue::CreateNumberValue(visible?placement.visible:0));
            status.Insert(L"total_items",json::JsonValue::CreateNumberValue(host.item_widths.size()));
            status.Insert(L"left",json::JsonValue::CreateNumberValue(visible?placement.left:0));
            status.Insert(L"width",json::JsonValue::CreateNumberValue(visible?placement.width:0));
            status.Insert(L"source_left",json::JsonValue::CreateNumberValue(visible?placement.source_left:0));
            status.Insert(L"height",json::JsonValue::CreateNumberValue(height));
            status.Insert(L"root_height",json::JsonValue::CreateNumberValue(geometry.height));
            status.Insert(L"scale",json::JsonValue::CreateNumberValue(sx));
            status.Insert(L"reason",json::JsonValue::CreateStringValue(!valid?L"layout_unavailable":!visible?L"insufficient_space":L"ready"));
            auto encoded=std::wstring(status.Stringify());
            if(encoded!=last_status){
                if(visible){
                    auto clip=xaml::Media::RectangleGeometry();
                    clip.Rect({static_cast<float>(placement.source_left),0,static_cast<float>(placement.width),static_cast<float>(height)});
                    host.panel.Clip(clip);
                    controls::Canvas::SetLeft(host.panel,placement.left-placement.source_left);
                    controls::Canvas::SetTop(host.panel,(geometry.height-height)/2.);
                }
                host.panel.Visibility(visible?xaml::Visibility::Visible:xaml::Visibility::Collapsed);
                last_status=encoded;host.layout_status=status;host.report();
            }
        }catch(...){
            try{
                host.panel.Visibility(xaml::Visibility::Collapsed);
                if(!last_status.empty()){
                    host.layout_status.Insert(L"reason",json::JsonValue::CreateStringValue(L"layout_unavailable"));
                    host.layout_status.Insert(L"visible_items",json::JsonValue::CreateNumberValue(0));host.report();
                }
            }catch(...){}
            last_status.clear();
        }
        updating=false;
    }
};
