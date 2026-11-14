#pragma once
// Pure geometry, in one XAML logical coordinate space. No Explorer dependencies.
#include <algorithm>
#include <cmath>
#include <vector>

namespace peripheral::taskbar {
struct Span { double left{}, right{}; };
struct Placement {
    double left{}, width{},source_left{};
    size_t visible{};
    bool left_side{};
};
inline Placement place(double width, double start_left, double tray_left,
                       bool prefer_left, bool centered, std::vector<Span> occupied,
                       std::vector<double> const& items, double gap=6.) {
    Placement result{};
    result.left_side=prefer_left&&centered;
    if(!std::isfinite(width)||!std::isfinite(start_left)||!std::isfinite(tray_left)
       ||width<=0||start_left<0||tray_left<=start_left||tray_left>width)return result;
    double begin=result.left_side?gap:start_left;
    double end=(result.left_side?start_left:tray_left)-gap;
    if(end<=begin)return result;
    std::sort(occupied.begin(),occupied.end(),[](auto a,auto b){return a.left<b.left;});
    std::vector<Span> free;
    double cursor=begin;
    for(auto block:occupied){
        if(!std::isfinite(block.left)||!std::isfinite(block.right)||block.right<block.left)return result;
        auto left=std::max(begin,block.left-gap),right=std::min(end,block.right+gap);
        if(right<=begin||left>=end)continue;
        if(left>cursor)free.push_back({cursor,left});
        cursor=std::max(cursor,right);
    }
    if(cursor<end)free.push_back({cursor,end});
    double total=0;for(auto item:items){if(!std::isfinite(item)||item<=0)return result;total+=item;}
    // Source order: descending preference on the left, ascending on the right.
    // Retain the outermost/highest priorities and anchor to the outer gap edge.
    for(auto space:free){
        double used=0;size_t count=0;
        for(size_t i=0;i<items.size();++i){
            auto item=items[result.left_side?i:items.size()-1-i];
            if(!std::isfinite(item)||item<=0||used+item>space.right-space.left)break;
            used+=item;++count;
        }
        if(count>0&&(count>result.visible||(!result.left_side&&count==result.visible))){
            result.visible=count;result.width=used;result.left=result.left_side?space.left:space.right-used;
            result.source_left=result.left_side?0:total-used;
        }
    }
    return result;
}
}
