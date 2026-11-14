#include "taskbar_layout.h"
#include <stdexcept>
#include <limits>
using namespace peripheral::taskbar;
void check(bool value){if(!value)throw std::runtime_error("taskbar layout regression");}
int main(){
    std::vector<Span> occupied{{0,150},{500,900},{1600,1920}};
    std::vector<double> items{120,90,80};
    auto left=place(1920,500,1600,true,true,occupied,items);
    check(left.left_side&&left.visible==3&&left.left==156&&left.width==290);
    auto right=place(1920,500,1600,false,true,occupied,items);
    check(!right.left_side&&right.visible==3&&right.left==1304);
    auto fallback=place(1920,0,1600,true,false,{{0,900},{1600,1920}},items);
    check(!fallback.left_side&&fallback.visible==3&&fallback.left==1304);
    auto small=place(1000,100,360,false,false,{{100,170},{360,1000}},items);
    check(small.visible==2&&small.width==170&&small.left==184&&small.source_left==120);
    auto none=place(1000,100,250,false,false,{{100,170},{260,1000}},items);
    check(none.visible==0&&none.width==0);
    check(place(1920,500,1600,true,true,occupied,{400,20}).visible==0);
    check(place(1920,500,1000,false,true,{{500,900},{1000,1920}},{20,400}).visible==0);
    check(place(1920,500,1600,true,true,{{0,490},{500,900},{1600,1920}},items).visible==0);
    // Resolution/DPI changes and tray expansion recompute from current bounds.
    for(double scale:{1.,1.25,1.5,2.}){
        auto scaled=place(1920/scale,500/scale,1600/scale,false,true,
            {{0,150/scale},{500/scale,900/scale},{1600/scale,1920/scale}},
            {120/scale,90/scale,80/scale},6/scale);
        check(scaled.visible==3&&std::abs(scaled.left-right.left/scale)<0.001);
    }
    check(place(1000,100,500,false,false,{{100,490},{500,1000}},items).visible==0);
    check(place(0,0,0,false,false,{},items).visible==0);
    check(place(1920,500,1600,false,true,{}, {std::numeric_limits<double>::quiet_NaN()}).visible==0);
    for(double screen:{640.,800.,1280.,1920.,3840.}){
        for(double start:{0.,100.,300.})for(double tray_width:{100.,250.,400.})for(bool left_side:{false,true}){
            double tray=screen-tray_width;
            std::vector<Span> blocks{{0,80},{start,start+210},{tray,screen}};
            auto p=place(screen,start,tray,left_side,start>0,blocks,items);
            if(!p.visible)continue;
            check(p.visible<=items.size()&&p.left>=0&&p.left+p.width<=tray-6);
            if(left_side&&start>0)check(p.left+p.width<=start-6);
            double expected=0;for(size_t i=0;i<p.visible;++i)expected+=items[p.left_side?i:items.size()-1-i];
            check(expected==p.width);
            for(auto block:blocks)check(p.left+p.width<=block.left-6||p.left>=block.right+6);
        }
    }
}
