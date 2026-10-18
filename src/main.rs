//! coderender —— 把代码渲染成一张图片。

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::Parser;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::ImageEncoder;
use tiny_skia::Pixmap;
use unicode_width::UnicodeWidthStr;

use coderender::cli::{Cli, Command, FontsArgs, PackArgs, PackCommand, PngCompression, RenderArgs, ThemesArgs};
use coderender::color::Rgba;
use coderender::document::Source;
use coderender::fonts::{self, FontSet};
use coderender::pack::{self, PackKind};
use coderender::render::{self, RenderOptions};
use coderender::syntax::{self, SyntaxRegistry};
use coderender::syntaxpack::{self, SyntaxPack};
use coderender::theme::{self, ThemePack, ThemeRegistry, WindowControls};

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("错误：{error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Render(args) => run_render(args),
        Command::Themes(args) => run_themes(args),
        Command::Fonts(args) => run_fonts(args),
        Command::Pack(args) => run_pack(args),
    }
}

fn run_render(args: RenderArgs) -> Result<()> {
    let total_start = Instant::now();

    // ---------------------------------------------------------------- 主题包
    let theme_pack = ThemeRegistry::load(args.theme_pack.as_deref())?;
    let palette = theme_pack
        .palette(&args.theme)
        .with_context(|| format!("解析配色预设 `{}` 失败", args.theme))?;
    let decoration = theme_pack
        .decoration(&args.window)
        .with_context(|| format!("解析窗口装饰 `{}` 失败", args.window))?;
    let canvas = args
        .canvas
        .as_deref()
        .map(Rgba::from_hex)
        .transpose()
        .context("解析 --canvas 颜色失败")?;

    // ---------------------------------------------------------------- 字体
    let font_init_start = Instant::now();
    let mut font_system = fonts::font_system();
    let font_init = font_init_start.elapsed();
    let font_set = fonts::resolve_checked(
        &font_system,
        args.font_mono.as_deref(),
        args.font_fallback.as_deref(),
    )?;

    // ---------------------------------------------------------------- 语法规则
    let syntax_start = Instant::now();
    let registry = SyntaxRegistry::load(args.syntax_pack.as_deref(), !args.no_user_packs)?;
    let syntax_load = syntax_start.elapsed();

    // ---------------------------------------------------------------- 输入
    let source = Source::read(args.input.as_deref(), args.tab_width)?;
    if source.lossy {
        eprintln!("提示：输入不是合法 UTF-8，非法字节已替换为 U+FFFD");
    }
    if source.text.is_empty() {
        eprintln!("提示：输入为空，将渲染一个空代码块");
    }

    let title = args.title.unwrap_or_else(|| source.default_title());
    let output = args.output.unwrap_or_else(|| source.default_output());
    if output.as_os_str() != "-"
        && output
            .extension()
            .map(|extension| !extension.eq_ignore_ascii_case("png"))
            .unwrap_or(false)
    {
        eprintln!("提示：输出扩展名不是 .png，仍会写入 PNG 数据");
    }

    let options = RenderOptions {
        scale: args.scale,
        font_size: args.font_size,
        line_height: args.line_height,
        padding: args.padding,
        line_numbers: !args.no_line_numbers,
        shadow: !args.no_shadow,
        canvas,
        title,
        lang: args.lang,
    };

    let rendered = render::render(
        &source,
        &palette,
        &decoration,
        &font_set,
        &mut font_system,
        &options,
        &registry,
    )
    .context("渲染失败")?;

    let encode_start = Instant::now();
    write_png(&rendered.pixmap, &output, args.png_compression)?;
    let encode_time = encode_start.elapsed();

    let stats = &rendered.stats;
    if output.as_os_str() == "-" {
        eprintln!(
            "已写入标准输出：{}x{}px，倍率 {}x",
            stats.width,
            stats.height,
            format_scale(args.scale)
        );
    } else {
        eprintln!(
            "已生成 {}：{}x{}px，倍率 {}x",
            output.display(),
            stats.width,
            stats.height,
            format_scale(args.scale)
        );
    }
    eprintln!(
        "  {} 行（{} 视觉行）· {} 个字形 · {}（来自 {}）· {} / {}",
        stats.lines,
        stats.visual_lines,
        stats.glyphs,
        stats.language,
        rendered.syntax_origin,
        stats.font_mono,
        stats.font_fallback
    );

    if args.verbose {
        eprintln!(
            "  耗时：字体 {:.1}ms · 语法表 {:.1}ms · 高亮 {:.1}ms · 整形 {:.1}ms · 绘制 {:.1}ms · 编码 {:.1}ms · 总计 {:.1}ms",
            ms(font_init),
            ms(syntax_load),
            ms(stats.highlight),
            ms(stats.shape),
            ms(stats.raster),
            ms(encode_time),
            ms(total_start.elapsed()),
        );
        eprintln!(
            "  配色：{}（{}）· 窗口：{}（{}）· 行号：{} · 阴影：{}",
            palette.name,
            palette.id,
            decoration.name,
            decoration.id,
            if options.line_numbers { "开" } else { "关" },
            if options.shadow { "开" } else { "关" },
        );
        eprintln!("  高亮规则来源：{}", registry.origins().join(" → "));
    }

    Ok(())
}

fn ms(duration: std::time::Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn format_scale(scale: f32) -> String {
    if scale.fract().abs() < f32::EPSILON {
        format!("{}", scale as u32)
    } else {
        format!("{scale}")
    }
}

fn write_png(pixmap: &Pixmap, path: &Path, compression: PngCompression) -> Result<()> {
    let (compression, filter) = match compression {
        PngCompression::Fast => (CompressionType::Fast, FilterType::Adaptive),
        PngCompression::Default => (CompressionType::Default, FilterType::Adaptive),
        PngCompression::Best => (CompressionType::Best, FilterType::Adaptive),
    };

    let to_stdout = path.as_os_str() == "-";
    let mut writer: Box<dyn Write> = if to_stdout {
        Box::new(std::io::stdout())
    } else {
        let file = File::create(path)
            .with_context(|| format!("无法创建输出文件 {}", path.display()))?;
        Box::new(BufWriter::new(file))
    };

    {
        let encoder = PngEncoder::new_with_quality(&mut writer, compression, filter);
        encoder
            .write_image(
                pixmap.data(),
                pixmap.width(),
                pixmap.height(),
                image::ExtendedColorType::Rgba8,
            )
            .context("PNG 编码失败")?;
    }
    writer.flush().context("写入 PNG 失败")?;
    Ok(())
}

// ---------------------------------------------------------------- themes

fn run_themes(args: ThemesArgs) -> Result<()> {
    if let Some(export) = args.export {
        std::fs::write(&export, theme::BUILTIN_PACK)
            .with_context(|| format!("无法写入 {}", export.display()))?;
        eprintln!("已导出内置主题包（Catppuccin）到 {}", export.display());
        return Ok(());
    }

    let registry = ThemeRegistry::load(args.theme_pack.as_deref())?;

    if registry.installed().is_empty() {
        eprintln!("（没有已安装的主题包，下面是内置主题包；`coderender pack install` 可安装更多）\n");
    }

    for (index, pack) in registry.installed().iter().enumerate() {
        print_pack(pack, if index == 0 { "已安装的主题包" } else { "" });
    }
    print_pack(registry.builtin(), "内置主题包");
    Ok(())
}

fn print_pack(pack: &ThemePack, label: &str) {
    println!(
        "主题包：{}（{}）{} {}",
        pack.pack.name.clone().unwrap_or_else(|| pack.pack.id.clone()),
        pack.pack.id,
        pack.pack
            .version
            .as_ref()
            .map(|version| format!("v{version}"))
            .unwrap_or_default(),
        if label.is_empty() {
            String::new()
        } else {
            format!("[{label}]")
        }
    );
    if let Some(homepage) = &pack.pack.homepage {
        println!("来源：{homepage}");
    }
    if let Some(description) = &pack.pack.description {
        println!("说明：{description}");
    }
    if let Some(license) = &pack.pack.license {
        println!("许可：{license}");
    }

    println!("\n代码配色预设（--theme）：");
    for id in pack.palette_ids() {
        let Ok(resolved) = pack.palette(id) else {
            continue;
        };
        println!(
            "  {} {} {} 背景 {} · 文字 {}",
            pad(id, 12),
            pad(&resolved.name, 22),
            pad(resolved.appearance.as_str(), 6),
            hex(resolved.background),
            hex(resolved.foreground),
        );
    }

    println!("\n窗口装饰预设（--window）：");
    for id in pack.decoration_ids() {
        let Ok(resolved) = pack.decoration(id) else {
            continue;
        };
        println!(
            "  {} {} 标题栏 {} 圆角 {} {}",
            pad(id, 12),
            pad(&resolved.name, 22),
            pad(&format!("{:.0}px", resolved.title_height), 6),
            pad(&format!("{:.0}px", resolved.corner_radius), 6),
            controls_summary(&resolved.controls),
        );
    }
    println!();
}

fn controls_summary(controls: &WindowControls) -> &'static str {
    match controls {
        WindowControls::TrafficLights { .. } => "macOS 红绿灯",
        WindowControls::Windows { .. } => "Windows 最小化 / 最大化 / 关闭",
        WindowControls::None => "无窗口按钮",
    }
}

fn pad(text: &str, width: usize) -> String {
    let display = UnicodeWidthStr::width(text);
    format!("{text}{}", " ".repeat(width.saturating_sub(display)))
}

fn hex(color: Rgba) -> String {
    if color.a == 255 {
        format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
    } else {
        format!(
            "#{:02x}{:02x}{:02x}{:02x}",
            color.r, color.g, color.b, color.a
        )
    }
}

// ---------------------------------------------------------------- fonts

fn run_fonts(args: FontsArgs) -> Result<()> {
    if args.license {
        print!("{}", fonts::FONT_LICENSE);
        return Ok(());
    }

    let system = fonts::font_system();
    let families = fonts::list_families(&system, !args.all);
    let filter = args.filter.as_deref().map(str::to_lowercase);
    let mut shown = 0usize;

    for family in &families {
        if let Some(filter) = &filter {
            if !family.to_lowercase().contains(filter) {
                continue;
            }
        }
        println!("{family}");
        shown += 1;
    }

    let default = FontSet::resolve(&system, None, None)?;
    eprintln!(
        "\n共 {} 个{}字体族（显示 {} 个）。默认：等宽 = {}，回退 = {}",
        families.len(),
        if args.all { "" } else { "等宽" },
        shown,
        default.mono,
        default.fallback
    );
    eprintln!("内置字体许可证：`coderender fonts --license`");
    Ok(())
}

// ---------------------------------------------------------------- pack

fn run_pack(args: PackArgs) -> Result<()> {
    match args.command {
        PackCommand::List { kind, verbose } => pack_list(kind.as_deref(), verbose),
        PackCommand::Install { paths, scope } => pack_install(&paths, &scope),
        PackCommand::Uninstall { id, kind } => pack_uninstall(&id, kind.as_deref()),
        PackCommand::New {
            name,
            kind,
            output,
        } => pack_new(&name, &kind, output.as_deref()),
        PackCommand::Build { dir, output } => pack_build(&dir, output.as_deref()),
        PackCommand::BuiltinExport { output } => pack_builtin_export(output.as_deref()),
    }
}

fn pack_list(kind: Option<&str>, verbose: bool) -> Result<()> {
    let kinds: Vec<PackKind> = match kind {
        Some(text) => vec![PackKind::parse(text).ok_or_else(|| {
            anyhow::anyhow!("无法识别的类型 `{text}`，可用：theme / syntax")
        })?],
        None => PackKind::ALL.to_vec(),
    };

    println!("扩展包目录：{}\n", pack::data_dir().join("packs").display());
    if let Some(machine) = pack::machine_dir() {
        println!("机器级目录：{}\n", machine.join("packs").display());
    }

    for kind in kinds {
        let installed = pack::installed(kind);
        println!("{}（{} 个）：", kind.label(), installed.len());
        if installed.is_empty() {
            println!("  （无）");
        }
        for item in &installed {
            println!(
                "  {:<16} {} [{}]",
                item.meta.id,
                item.title(),
                item.scope.label()
            );
            if let Some(description) = &item.meta.description {
                println!("  {:<16}   {}", "", description);
            }
            if verbose {
                println!("  {:<16}   路径 {}", "", item.path.display());
                if let Some(license) = &item.meta.license {
                    println!("  {:<16}   许可 {license}", "");
                }
                match kind {
                    PackKind::Theme => {
                        if let Ok(pack) = ThemePack::load(Some(&item.path.join(pack::PACK_FILE_NAME))) {
                            println!(
                                "  {:<16}   配色 {} · 窗口 {}",
                                "",
                                pack.palette_ids().join(", "),
                                pack.decoration_ids().join(", ")
                            );
                        }
                    }
                    PackKind::Syntax => {
                        if let Ok(pack) = SyntaxPack::load(&item.path) {
                            match pack.build_syntax_sets() {
                                Ok(sets) => {
                                    let mut languages: Vec<String> = sets
                                        .iter()
                                        .flat_map(syntaxpack::describe_syntaxes)
                                        .map(|(name, _)| name)
                                        .collect();
                                    languages.sort_by_key(|name| name.to_lowercase());
                                    languages.dedup();
                                    println!(
                                        "  {:<16}   语法 {} 种：{}",
                                        "",
                                        languages.len(),
                                        languages.join(", ")
                                    );
                                }
                                Err(error) => {
                                    println!("  {:<16}   （语法加载失败：{error}）", "");
                                }
                            }
                        }
                    }
                }
            }
        }
        println!();
    }

    // 内置包
    println!("内置（无需安装，可被安装的包覆盖）：");
    let builtin = ThemeRegistry::load(None)
        .ok()
        .map(|registry| registry.builtin().palette_ids().join(", "))
        .unwrap_or_default();
    println!("  {:<16} Catppuccin（配色：{builtin}）", "themes");
    let languages = SyntaxRegistry::load(None, false)
        .map(|registry| registry.language_names().len())
        .unwrap_or(0);
    println!(
        "  {:<16} builtin-syntaxes（{languages} 种语言/语法，`coderender pack builtin-export` 可导出）",
        "syntax"
    );
    Ok(())
}

fn pack_install(paths: &[std::path::PathBuf], scope_text: &str) -> Result<()> {
    let scope = pack::Scope::parse(scope_text)
        .ok_or_else(|| anyhow::anyhow!("无法识别的安装作用域 `{scope_text}`，可用：user / machine"))?;
    let mut installed = Vec::new();
    for path in paths {
        let meta = pack::install(path, scope)?;
        println!(
            "已安装{} `{}`（{}）→ {} [{}]",
            meta.kind.label(),
            meta.id,
            meta.name,
            pack::packs_dir_in(&pack_root_for(scope), meta.kind)
                .join(&meta.id)
                .display(),
            scope.label()
        );
        installed.push(meta);
    }
    eprintln!(
        "共安装 {} 个包（{}）。主题包安装后可直接 `--theme <id>` 使用；代码高亮扩展包安装后可直接 `--lang <名称>` 使用。",
        installed.len(),
        scope.label()
    );
    if scope == pack::Scope::Machine {
        eprintln!("机器级插件对所有用户生效，卸载时也要用 --scope machine。");
    }
    Ok(())
}

fn pack_root_for(scope: pack::Scope) -> std::path::PathBuf {
    match scope {
        pack::Scope::User => pack::data_dir(),
        pack::Scope::Machine => pack::machine_dir().unwrap_or_else(pack::data_dir),
    }
}

fn pack_uninstall(id: &str, kind: Option<&str>) -> Result<()> {
    let kinds: Vec<PackKind> = match kind {
        Some(text) => vec![PackKind::parse(text).ok_or_else(|| {
            anyhow::anyhow!("无法识别的类型 `{text}`，可用：theme / syntax")
        })?],
        None => PackKind::ALL.to_vec(),
    };

    for kind in kinds {
        if pack::find_installed(kind, id).is_some() {
            let meta = pack::uninstall(kind, id)?;
            println!("已卸载{} `{}`", kind.label(), meta.id);
            return Ok(());
        }
    }

    let candidates: Vec<String> = PackKind::ALL
        .iter()
        .flat_map(|kind| pack::installed_ids(*kind))
        .collect();
    anyhow::bail!(
        "没有安装过包 `{id}`{}。可用 `coderender pack list` 查看已安装的包。",
        coderender::dym::did_you_mean(id, &candidates, 3)
    );
}

fn pack_new(name: &str, kind: &str, output: Option<&Path>) -> Result<()> {
    let kind = PackKind::parse(kind)
        .ok_or_else(|| anyhow::anyhow!("无法识别的类型 `{kind}`，可用：theme / syntax"))?;
    let dir = pack::create_template(name, kind, output)?;
    println!("已生成{}开发模板：{}", kind.label(), dir.display());
    eprintln!(
        "编辑其中的 {} 后用 `coderender pack install {}` 安装，或用 `coderender pack build {}` 打包分发。",
        pack::PACK_FILE_NAME,
        dir.display(),
        dir.display()
    );
    Ok(())
}

fn pack_build(dir: &Path, output: Option<&Path>) -> Result<()> {
    let out = pack::build_distribution(dir, output)?;
    let meta = pack::read_meta(dir)?;
    println!(
        "已打包{} `{}` → {}（{} 字节）",
        meta.kind.label(),
        meta.id,
        out.display(),
        std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0)
    );
    eprintln!("分发：把这个 .{} 发给别人，对方执行 `coderender pack install {}`", pack::PACK_EXTENSION, out.display());
    Ok(())
}

fn pack_builtin_export(output: Option<&Path>) -> Result<()> {
    let dir = output
        .map(Path::to_path_buf)
        .unwrap_or_else(|| Path::new("builtin-syntaxes").to_path_buf());
    if dir.exists() {
        anyhow::bail!("目录 {} 已存在", dir.display());
    }
    SyntaxRegistry::export_builtin(&dir)?;
    let syntaxes = syntax::SyntaxRegistry::load(None, false)
        .map(|registry| registry.language_names().len())
        .unwrap_or(0);
    println!(
        "已导出内置代码高亮扩展包到 {}（{} 个语言/语法，{}）",
        dir.display(),
        syntaxes,
        dir.join("syntaxes").join("syntaxes.packdump").display()
    );
    eprintln!(
        "可以直接 `coderender pack install {}` 安装它，再按需修改语法文件后重新打包。",
        dir.display()
    );
    Ok(())
}
