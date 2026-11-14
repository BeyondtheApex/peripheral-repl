// Isolated XAML integration fixture. Never discovers or modifies Explorer.
#define PERIPHERAL_LIBRARY
#include "xaml_host.cpp"
#include "taskbar_placement.h"
#include "taskbar_monitor.h"
#include "taskbar_roots.h"
#include <iostream>
#include <chrono>
void require(bool value,char const* message){if(!value)throw std::runtime_error(message);}
void arrange(controls::Grid const& root){
    root.UpdateLayout();
    MSG message{};
    while(PeekMessageW(&message,nullptr,0,0,PM_REMOVE)){TranslateMessage(&message);DispatchMessageW(&message);}
    root.UpdateLayout();
}
int main(){
    try{
        winrt::init_apartment(winrt::apartment_type::single_threaded);
        auto manager=hosting::WindowsXamlManager::InitializeForCurrentThread();
        WNDCLASSW wc{};wc.lpfnWndProc=DefWindowProcW;wc.hInstance=GetModuleHandleW(nullptr);wc.lpszClassName=L"Shell_TrayWnd";RegisterClassW(&wc);
        auto hwnd=CreateWindowExW(WS_EX_NOACTIVATE|WS_EX_TOOLWINDOW,wc.lpszClassName,L"",WS_POPUP,-20000,-20000,1200,48,nullptr,nullptr,wc.hInstance,nullptr);
        hosting::DesktopWindowXamlSource source;
        auto native=source.as<IDesktopWindowXamlSourceNative>();winrt::check_hresult(native->AttachToWindow(hwnd));
        HWND island{};winrt::check_hresult(native->get_WindowHandle(&island));
        SetWindowPos(island,nullptr,0,0,1200,48,SWP_NOACTIVATE|SWP_NOZORDER|SWP_SHOWWINDOW);
        ShowWindow(hwnd,SW_SHOWNOACTIVATE);
        controls::Grid root;root.Width(1200);root.Height(48);
        auto add=[&](xaml::FrameworkElement const& item,double left,double width){
            item.HorizontalAlignment(xaml::HorizontalAlignment::Left);
            item.VerticalAlignment(xaml::VerticalAlignment::Top);
            item.Width(width);item.Height(48);item.Margin({left,0,0,0});root.Children().Append(item);
        };
        controls::Button start;start.Name(L"StartButton");add(start,400,48);
        controls::Button apps;add(apps,448,252);
        controls::Button widgets;add(widgets,0,140);
        controls::Border tray;add(tray,1040,160);
        source.Content(root);arrange(root);
        // Reproduce two tray nodes on one XAML thread with identical dimensions.
        wc.lpszClassName=L"Shell_SecondaryTrayWnd";RegisterClassW(&wc);
        auto secondary=CreateWindowExW(WS_EX_NOACTIVATE|WS_EX_TOOLWINDOW,wc.lpszClassName,L"",WS_POPUP,-18000,-20000,1200,48,nullptr,nullptr,wc.hInstance,nullptr);
        hosting::DesktopWindowXamlSource secondary_source;
        auto secondary_native=secondary_source.as<IDesktopWindowXamlSourceNative>();
        winrt::check_hresult(secondary_native->AttachToWindow(secondary));
        HWND secondary_island{};winrt::check_hresult(secondary_native->get_WindowHandle(&secondary_island));
        SetWindowPos(secondary_island,nullptr,0,0,1200,48,SWP_NOACTIVATE|SWP_NOZORDER|SWP_SHOWWINDOW);
        ShowWindow(secondary,SW_SHOWNOACTIVATE);
        controls::Grid secondary_root;secondary_root.Width(1200);secondary_root.Height(48);
        controls::Border secondary_tray;secondary_root.Children().Append(secondary_tray);
        secondary_source.Content(secondary_root);arrange(secondary_root);
        require(!peripheral::taskbar::taskbar_for_thread().bar,"fixture must reproduce shared-thread ambiguity");
        TaskbarRoots roots;
        roots.add(2,secondary_source);roots.add(1,source); // Secondary discovered first.
        auto selected=roots.match(tray),other=roots.match(secondary_tray);
        require(selected.exact&&selected.bar==hwnd&&selected.island==island,"primary source must resolve its own HWND");
        require(other.exact&&other.bar==secondary&&other.island==secondary_island,"secondary must not borrow primary HWND");
        require(!peripheral::taskbar::primary_window(other.bar),"secondary taskbar must remain excluded");
        roots.remove(1);
        require(!roots.match(tray).bar,"removed source must not fall back to the other root");
        roots.add(1,source);
        require(roots.match(tray).bar==hwnd,"source re-registration must recover");
        secondary_source.Close();DestroyWindow(secondary);
        std::cout<<"Shared-thread equal-size XAML source identity checks passed\n";
        Host host;host.embedded=true;host.make_panel();
        host.dir=std::filesystem::temp_directory_path()/(L"peripheral-layout-test-"+std::to_wstring(GetCurrentProcessId()));
        std::filesystem::create_directories(host.dir);
        controls::Border texture_area;
        texture_area.Background(xaml::Media::SolidColorBrush(winrt::Windows::UI::Color{255,32,40,48}));
        host.panel.Children().Append(texture_area);
        host.item_widths={100,90,80};host.left_order=true;
        auto set_frame=[&]{
            host.logical_width=270;host.logical_height=36;
            host.pixel_width=static_cast<uint32_t>(270*host.panel.CompositionScaleX());
            host.pixel_height=static_cast<uint32_t>(36*host.panel.CompositionScaleY());
            host.panel.Width(270);host.panel.Height(36);
        };
        bool centered=true,primary=true;
        auto count=[&]{return host.layout_status.GetNamedNumber(L"visible_items");};
        auto before=root.Children().Size();
        {
            TaskbarPlacement placement(host,tray,true,[&]{return centered;},[&]{return primary;});
            arrange(root);set_frame();placement.refresh();arrange(root);
            require(count()==2,"left gap must hide third item");
            auto left_before=host.layout_status.GetNamedNumber(L"left");
            host.pixel_width=540;host.pixel_height=72;
            // Exercise the real DXGI transform with a 2x producer, independent
            // of the test desktop's own DPI. WARP requires no particular GPU.
            winrt::check_hresult(D3D11CreateDevice(nullptr,D3D_DRIVER_TYPE_WARP,nullptr,D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                nullptr,0,D3D11_SDK_VERSION,host.device.put(),nullptr,host.context.put()));
            winrt::com_ptr<IDXGIFactory2> factory;
            winrt::check_hresult(CreateDXGIFactory1(__uuidof(IDXGIFactory2),factory.put_void()));
            DXGI_SWAP_CHAIN_DESC1 desc{};desc.Width=540;desc.Height=72;desc.Format=DXGI_FORMAT_B8G8R8A8_UNORM;
            desc.SampleDesc.Count=1;desc.BufferUsage=DXGI_USAGE_RENDER_TARGET_OUTPUT;desc.BufferCount=2;
            desc.Scaling=DXGI_SCALING_STRETCH;desc.SwapEffect=DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL;desc.AlphaMode=DXGI_ALPHA_MODE_PREMULTIPLIED;
            winrt::check_hresult(factory->CreateSwapChainForComposition(host.device.get(),&desc,nullptr,host.swap.put()));
            winrt::check_hresult(host.panel.as<ISwapChainPanelNative>()->SetSwapChain(host.swap.get()));
            host.update_scale();
            DXGI_MATRIX_3X2_F matrix{};winrt::check_hresult(host.swap.as<IDXGISwapChain2>()->GetMatrixTransform(&matrix));
            require(matrix._11==0.5f&&matrix._22==0.5f&&host.panel.Width()==270&&host.panel.Height()==36,
                "actual swapchain must retain logical dimensions at mixed DPI");
            placement.refresh();arrange(root);
            require(count()==2&&host.layout_status.GetNamedNumber(L"left")==left_before,"producer DPI must not alter consumer layout");
            require(host.input_scale_x()==2&&host.input_scale_y()==2,"input must map to producer pixels at mixed DPI");
            primary=false;placement.refresh();arrange(root);
            require(count()==0&&host.panel.Visibility()==xaml::Visibility::Collapsed,"old primary must hide immediately");
            primary=true;placement.refresh();arrange(root);
            require(count()==2,"primary selection must recover");
            require(host.panel.Clip()!=nullptr,"visible prefix needs clipping");
            auto r=peripheral::taskbar::bounds(host.panel,root);
            auto clip=host.panel.Clip().as<xaml::Media::RectangleGeometry>().Rect();
            require(std::abs(r.X-146)<0.1,"left group must hug widgets, not Start");
            require(r.X>=146&&r.X+clip.Width<=394.1,"left content overlaps widgets or Start");
            bool inside=false;
            for(auto hit:xaml::Media::VisualTreeHelper::FindElementsInHostCoordinates(
                winrt::Windows::Foundation::Point{r.X+10,r.Y+10},root))inside=inside||hit==host.panel;
            require(inside,"visible content must receive pointer input");
            for(auto hit:xaml::Media::VisualTreeHelper::FindElementsInHostCoordinates(
                winrt::Windows::Foundation::Point{r.X+clip.Width+10,r.Y+10},root))
                require(hit!=host.panel,"clipped tail must not intercept system input");
            centered=false;host.left_order=false;placement.refresh();arrange(root);
            require(count()==3,"left aligned must fall back to right");
            require(host.layout_status.GetNamedString(L"effective_side")==L"right","fallback side");
            // Tray templates may expose translated/clipped children on expansion,
            // while the tray's outer layout footprint remains unchanged.
            auto stable_right=host.layout_status.GetNamedNumber(L"left");
            controls::Button tray_content;
            tray_content.Width(240);tray_content.Height(48);
            tray_content.HorizontalAlignment(xaml::HorizontalAlignment::Left);
            xaml::Media::TranslateTransform tray_motion;tray_motion.X(-80);
            tray_content.RenderTransform(tray_motion);tray.Child(tray_content);
            arrange(root);placement.refresh();
            require(count()==3&&host.layout_status.GetNamedNumber(L"left")==stable_right,
                "tray template expansion must not shift the group when outer bounds are unchanged");
            tray.Child(nullptr);arrange(root);placement.refresh();
            require(host.layout_status.GetNamedNumber(L"left")==stable_right,
                "tray template collapse must not shift the group");
            tray.Margin({850,0,0,0});tray.Width(350);arrange(root);placement.refresh();
            require(count()==1,"expanded tray must reduce visible prefix");
            auto right_bounds=peripheral::taskbar::bounds(host.panel,root);
            auto right_clip=host.panel.Clip().as<xaml::Media::RectangleGeometry>().Rect();
            require(right_clip.X==190&&right_clip.Width==80,"right must retain highest-priority suffix");
            require(std::abs(right_bounds.X+right_clip.X+right_clip.Width-844)<0.1,"right group must hug tray");
            bool right_hit=false;
            for(auto hit:xaml::Media::VisualTreeHelper::FindElementsInHostCoordinates(
                winrt::Windows::Foundation::Point{right_bounds.X+right_clip.X+10,right_bounds.Y+10},root))right_hit=right_hit||hit==host.panel;
            require(right_hit,"visible right suffix must receive pointer input");
            for(auto hit:xaml::Media::VisualTreeHelper::FindElementsInHostCoordinates(
                winrt::Windows::Foundation::Point{right_bounds.X+right_clip.X-10,right_bounds.Y+10},root))
                require(hit!=host.panel,"hidden low-priority prefix must not intercept system input");
            tray.Margin({750,0,0,0});tray.Width(450);arrange(root);placement.refresh();
            require(count()==0&&host.panel.Visibility()==xaml::Visibility::Collapsed,"no space must hide all");
            tray.Margin({1040,0,0,0});tray.Width(160);arrange(root);placement.refresh();
            require(count()==3,"freed space must restore items");
            start.Name(L"UnknownStart");arrange(root);placement.refresh();
            require(count()==0,"unknown layout must fail closed");
            start.Name(L"StartButton");arrange(root);placement.refresh();
            require(count()==3,"valid layout must recover");
            tray.HorizontalAlignment(xaml::HorizontalAlignment::Right);tray.Margin({0,0,0,0});
            root.Width(780);arrange(root);
            require(count()==0,"resolution shrink must update without an explicit refresh");
            root.Width(1600);arrange(root);
            require(count()==3,"resolution growth must restore content on LayoutUpdated");
            auto begin=std::chrono::steady_clock::now();
            for(int i=0;i<100;++i)placement.refresh();
            std::cout<<"100 unchanged XAML layouts: "<<std::chrono::duration_cast<std::chrono::microseconds>(std::chrono::steady_clock::now()-begin).count()<<" us\n";
        }
        require(root.Children().Size()==before,"detach leaked overlay");
        std::filesystem::remove(host.dir/L"consumer.json");std::filesystem::remove(host.dir);
        source.Close();DestroyWindow(hwnd);manager.Close();
        std::cout<<"XAML layout integration passed\n";
        return 0;
    }catch(winrt::hresult_error const& e){std::cerr<<winrt::to_string(e.message())<<" ("<<std::hex<<e.code().value<<")\n";return 1;}
    catch(std::exception const& e){std::cerr<<e.what()<<"\n";return 1;}
}
