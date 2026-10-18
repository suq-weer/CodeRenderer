//! coderender —— 把代码渲染成一张图片。
//!
//! 库结构：
//! * [`theme`] 主题包（配色预设 + 窗口装饰预设），内置 Catppuccin；
//! * [`fonts`] 系统字体解析 + 内置 JetBrainsMono Nerd Font；
//! * [`syntax`] syntect 语法高亮 → 带颜色的字节区间；
//! * [`document`] 输入读取与规范化；
//! * [`render`] 排版、几何计算与矢量绘制；
//! * [`cli`] 命令行定义。

pub mod cli;
pub mod color;
pub mod document;
pub mod dym;
pub mod fonts;
pub mod pack;
pub mod render;
pub mod syntax;
pub mod syntaxpack;
pub mod theme;
