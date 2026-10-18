//! 命令行定义。

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "coderender",
    version,
    about = "把代码渲染成图片：主题包 + 窗口装饰 + 任意倍率矢量渲染",
    long_about = "coderender 读取代码文件（或标准输入），按主题包中的配色与窗口装饰预设渲染为 PNG。\n\
                  默认使用 Catppuccin Mocha 配色 + macOS 红绿灯窗口。",
    after_help = "示例：\n  coderender render src/main.rs -o main.png --theme mocha --window windows --scale 2\n  cat foo.py | coderender render --lang python -o foo.png\n  coderender themes --export my-pack.toml"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)]
pub enum Command {
    /// 把代码渲染为 PNG
    Render(RenderArgs),
    /// 列出主题包中的配色预设与窗口装饰预设
    Themes(ThemesArgs),
    /// 列出已安装字体（默认只列等宽字体）
    Fonts(FontsArgs),
    /// 管理主题包 / 代码高亮扩展包（安装、卸载、打包、生成模板）
    Pack(PackArgs),
}

#[derive(Debug, Args)]
pub struct PackArgs {
    #[command(subcommand)]
    pub command: PackCommand,
}

#[derive(Debug, Subcommand)]
pub enum PackCommand {
    /// 列出已安装的主题包与代码高亮扩展包
    List {
        /// 只列某一类：theme / syntax
        #[arg(long, value_name = "KIND")]
        kind: Option<String>,
        /// 同时列出包内的配色/语法明细
        #[arg(short, long)]
        verbose: bool,
    },
    /// 安装扩展包（开发目录或 .crpack 分发包）
    Install {
        /// 要安装的目录 / .crpack 文件，可以一次装多个
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// 安装作用域：user（当前用户，默认）或 machine（所有用户，需要管理员权限）
        #[arg(long, value_name = "SCOPE", default_value = "user")]
        scope: String,
    },
    /// 卸载扩展包
    Uninstall {
        /// 包 ID（也接受包名）
        #[arg(required = true)]
        id: String,
        /// 指定类型：theme / syntax；省略时自动判断
        #[arg(long, value_name = "KIND")]
        kind: Option<String>,
    },
    /// 生成扩展包开发模板
    New {
        /// 包名（会转成小写连字符 ID）
        name: String,
        /// 类型：theme / syntax
        #[arg(long, value_name = "KIND", default_value = "theme")]
        kind: String,
        /// 输出目录，默认用包 ID
        #[arg(short, long, value_name = "DIR")]
        output: Option<PathBuf>,
    },
    /// 把开发目录打包成 .crpack 分发文件
    Build {
        /// 扩展包目录
        dir: PathBuf,
        /// 输出文件，默认 `<id>-<version>.crpack`
        #[arg(short, long, value_name = "FILE")]
        output: Option<PathBuf>,
    },
    /// 把内置的代码高亮规则导出成一个扩展包（可安装、可修改）
    BuiltinExport {
        /// 输出目录，默认 `builtin-syntaxes`
        #[arg(short, long, value_name = "DIR")]
        output: Option<PathBuf>,
    },
}

#[derive(Debug, Args)]
pub struct RenderArgs {
    /// 输入文件；省略或 `-` 表示从标准输入读取
    pub input: Option<PathBuf>,

    /// 输出 PNG 路径；`-` 表示写到标准输出
    #[arg(short, long, value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// 代码配色预设 ID（如 latte / frappe / macchiato / mocha）
    #[arg(short, long, value_name = "ID", default_value = crate::theme::DEFAULT_PALETTE)]
    pub theme: String,

    /// 自定义主题包（TOML）；省略时使用内置 Catppuccin 与已安装的主题包
    #[arg(long, value_name = "PATH")]
    pub theme_pack: Option<PathBuf>,

    /// 代码高亮扩展包（目录或 .crpack）；省略时使用已安装的扩展包与内置语法集
    #[arg(long, value_name = "PATH")]
    pub syntax_pack: Option<PathBuf>,

    /// 不加载用户已安装的扩展包（只用 --syntax-pack 与内置规则）
    #[arg(long)]
    pub no_user_packs: bool,

    /// 窗口装饰预设 ID（如 macos / windows / none）
    #[arg(short, long, value_name = "ID", default_value = crate::theme::DEFAULT_DECORATION)]
    pub window: String,

    /// 清晰度倍率：1x 为刚好容纳代码的尺寸，2x 以上为矢量超分渲染
    #[arg(short, long, value_name = "N", default_value = "1", value_parser = parse_scale)]
    pub scale: f32,

    /// 代码等宽字体名（系统已安装，或内置 JetBrainsMono Nerd Font）
    #[arg(long, value_name = "NAME")]
    pub font_mono: Option<String>,

    /// 非等宽回退字体名（用于中文等字符）
    #[arg(long, value_name = "NAME")]
    pub font_fallback: Option<String>,

    /// 字号（逻辑像素）
    #[arg(long, value_name = "PX", default_value_t = 14.0, value_parser = parse_font_size)]
    pub font_size: f32,

    /// 行高倍数
    #[arg(long, value_name = "MULT", default_value_t = 1.55, value_parser = parse_line_height)]
    pub line_height: f32,

    /// 代码区内边距（逻辑像素），默认取窗口装饰预设
    #[arg(long, value_name = "PX")]
    pub padding: Option<f32>,

    /// Tab 展开的空格数
    #[arg(long, value_name = "N", default_value_t = 4, value_parser = parse_tab_width)]
    pub tab_width: usize,

    /// 不显示行号
    #[arg(long = "no-line-numbers")]
    pub no_line_numbers: bool,

    /// 不绘制窗口阴影
    #[arg(long)]
    pub no_shadow: bool,

    /// 画布底色（#rrggbb[aa]），默认透明
    #[arg(long, value_name = "COLOR")]
    pub canvas: Option<String>,

    /// 标题栏文字，默认为文件名
    #[arg(long, value_name = "TEXT")]
    pub title: Option<String>,

    /// 强制指定语法（如 rust / python / json）
    #[arg(long, value_name = "LANG")]
    pub lang: Option<String>,

    /// PNG 压缩级别
    #[arg(long, value_enum, default_value_t = PngCompression::Fast)]
    pub png_compression: PngCompression,

    /// 打印每个阶段的耗时与字体信息
    #[arg(short, long)]
    pub verbose: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum PngCompression {
    Fast,
    Default,
    Best,
}

#[derive(Debug, Args)]
pub struct ThemesArgs {
    /// 自定义主题包（TOML）；省略时展示内置 Catppuccin
    #[arg(long, value_name = "PATH")]
    pub theme_pack: Option<PathBuf>,

    /// 把内置主题包导出为 TOML 模板
    #[arg(long, value_name = "PATH")]
    pub export: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct FontsArgs {
    /// 列出全部字体族（默认只列等宽字体）
    #[arg(long)]
    pub all: bool,

    /// 按名称过滤（不区分大小写）
    #[arg(long, value_name = "TEXT")]
    pub filter: Option<String>,

    /// 打印内置字体的 SIL Open Font License
    #[arg(long)]
    pub license: bool,
}

fn parse_scale(value: &str) -> Result<f32, String> {
    let cleaned = value.trim().trim_end_matches(['x', 'X']);
    let scale: f32 = cleaned
        .parse()
        .map_err(|_| format!("倍率 `{value}` 不是数字（示例：--scale 2）"))?;
    if !(1.0..=4.0).contains(&scale) {
        return Err(format!("倍率 {scale} 超出范围：需在 1x ~ 4x 之间"));
    }
    Ok(scale)
}

fn parse_font_size(value: &str) -> Result<f32, String> {
    let size: f32 = value
        .parse()
        .map_err(|_| format!("字号 `{value}` 不是数字"))?;
    if !(6.0..=200.0).contains(&size) {
        return Err("字号需在 6 ~ 200 之间".to_string());
    }
    Ok(size)
}

fn parse_line_height(value: &str) -> Result<f32, String> {
    let mult: f32 = value
        .parse()
        .map_err(|_| format!("行高倍数 `{value}` 不是数字"))?;
    if !(1.0..=3.0).contains(&mult) {
        return Err("行高倍数需在 1.0 ~ 3.0 之间".to_string());
    }
    Ok(mult)
}

fn parse_tab_width(value: &str) -> Result<usize, String> {
    match value.parse::<usize>() {
        Ok(width) if (1..=16).contains(&width) => Ok(width),
        Ok(_) => Err("Tab 宽度需在 1 ~ 16 之间".to_string()),
        Err(_) => Err(format!("Tab 宽度 `{value}` 不是整数")),
    }
}
