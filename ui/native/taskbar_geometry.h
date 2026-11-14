#pragma once
#include "taskbar_layout.h"
#include <winrt/Windows.UI.Xaml.Media.h>
#include <winrt/Windows.UI.Xaml.Automation.h>

namespace peripheral::taskbar {
namespace x=winrt::Windows::UI::Xaml;
struct Geometry {
    bool valid=false;
    double width{},height{},start_left{},tray_left{};
    std::vector<Span> occupied;
    size_t visited{};
};
inline winrt::Windows::Foundation::Rect bounds(x::FrameworkElement const& element,x::UIElement const& root){
    return element.TransformToVisual(root).TransformBounds({0,0,static_cast<float>(element.ActualWidth()),static_cast<float>(element.ActualHeight())});
}
inline bool scan(x::DependencyObject const& node,x::FrameworkElement const& root,
                 x::UIElement const& own,x::FrameworkElement const& tray_anchor,
                 Geometry& out,bool& start,unsigned depth=0){
    if(depth>32||++out.visited>4096)return false;
    if(node==own)return true;
    // Reserve the tray's entire outer footprint in measure(). Descendants can
    // animate/translate outside it when the overflow flyout opens, without
    // changing taskbar layout; counting them again makes our group jump.
    if(node==tray_anchor)return true;
    if(auto element=node.try_as<x::FrameworkElement>()){
        if(element.Visibility()!=x::Visibility::Visible)return true;
        auto name=std::wstring(element.Name());
        auto type=std::wstring(winrt::get_class_name(element));
        auto automation=std::wstring(x::Automation::AutomationProperties::GetAutomationId(element));
        bool is_start=name==L"StartButton"||automation==L"StartButton";
        // Ignore decorative descendants. Reserve full interactive controls,
        // including search, widgets, task buttons, overflow and third-party controls.
        bool control=element.try_as<x::Controls::Control>()!=nullptr;
        bool button=type.find(L"Button")!=std::wstring::npos
            ||type.find(L"ExperienceToggle")!=std::wstring::npos;
        bool tray=type==L"SystemTray.SystemTrayFrame";
        if(is_start||control||button||tray){
            auto rect=bounds(element,root);
            if(!std::isfinite(rect.X)||!std::isfinite(rect.Y)||!std::isfinite(rect.Width)||!std::isfinite(rect.Height))return false;
            if(rect.Width>0&&rect.Height>0&&rect.Y<out.height&&rect.Y+rect.Height>0){
                // Stretch containers aren't evidence of an occupied control.
                if(tray||button||is_start||rect.Width<out.width*0.5)
                    out.occupied.push_back({rect.X,rect.X+rect.Width});
                if(is_start){out.start_left=rect.X;start=true;}
            }
        }
    }
    auto count=x::Media::VisualTreeHelper::GetChildrenCount(node);
    for(int i=0;i<count;++i)
        if(!scan(x::Media::VisualTreeHelper::GetChild(node,i),root,own,tray_anchor,out,start,depth+1))return false;
    return true;
}
inline Geometry measure(x::Controls::Grid const& root,x::FrameworkElement const& tray,x::UIElement const& own){
    Geometry out;
    if(!root.XamlRoot()||root.XamlRoot()!=tray.XamlRoot())return out;
    out.width=root.ActualWidth();out.height=root.ActualHeight();
    if(!std::isfinite(out.width)||!std::isfinite(out.height)||out.width<=0||out.height<=0||out.height>160)return out;
    auto t=bounds(tray,root);out.tray_left=t.X;
    if(t.Width<=0||t.X<0||t.X>=out.width)return out;
    bool start=false;
    out.valid=scan(root,root,own,tray,out,start)&&start;
    // Protect the entire system tray end, including clock/show-desktop padding.
    out.occupied.push_back({out.tray_left,out.width});
    return out;
}
}
