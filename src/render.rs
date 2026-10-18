//! 渲染管线：排版 → 几何计算 → 矢量绘制（窗口装饰 + 代码字形）→ 位图。
//!
//! 性能设计要点：
//! * 整段代码只做 **一次** 整形（`shape_until_scroll`），不按行创建 Buffer；
//! * 字形位图通过 `SwashCache` 复用，按 (font, glyph, size, subpixel) 缓存；
//! * 混合使用整数运算直接写 premultiplied RGBA，避免逐像素分配；
//! * 倍率 `scale` 只影响栅格化尺寸与几何，不做位图放大 —— 2x/3x 是真正的矢量重绘；
//! * 阴影在 1/4 分辨率上做，且只合成窗口外的边缘区域。

use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use cosmic_text::{
    Attrs, Buffer, FontSystem, Metrics, Shaping, Style as FontStyle, SwashCache, Weight, Wrap,
};
use tiny_skia::{
    Color as SkColor, FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Rect, Stroke,
    Transform,
};

use crate::color::Rgba;
use crate::document::Source;
use crate::fonts::FontSet;
use crate::syntax::{Highlighter, SyntaxRegistry};
use crate::theme::{ResolvedDecoration, ResolvedPalette, TitleAlign, WindowControls};

/// 单张图的像素上限（约 1.2 GB RGBA），超过时提示降低倍率。
const MAX_PIXELS: u64 = 300_000_000;

pub struct RenderOptions {
    pub scale: f32,
    pub font_size: f32,
    pub line_height: f32,
    pub padding: Option<f32>,
    pub line_numbers: bool,
    pub shadow: bool,
    pub canvas: Option<Rgba>,
    pub title: String,
    pub lang: Option<String>,
}

#[derive(Debug, Default)]
pub struct Stats {
    pub width: u32,
    pub height: u32,
    pub lines: usize,
    pub visual_lines: usize,
    pub glyphs: usize,
    pub language: String,
    pub font_mono: String,
    pub font_fallback: String,
    pub syntax_load: Duration,
    pub highlight: Duration,
    pub shape: Duration,
    pub raster: Duration,
}

pub struct Rendered {
    pub pixmap: Pixmap,
    pub stats: Stats,
    /// 本次使用的语法来源（内置 / 扩展包名）。
    pub syntax_origin: String,
}

pub fn render(
    source: &Source,
    palette: &ResolvedPalette,
    decoration: &ResolvedDecoration,
    fonts: &FontSet,
    font_system: &mut FontSystem,
    options: &RenderOptions,
    registry: &SyntaxRegistry,
) -> Result<Rendered> {
    let scale = options.scale;
    let line_height_logical = (options.font_size * options.line_height)
        .round()
        .max(options.font_size + 1.0);
    let font_px = options.font_size * scale;
    let line_px = line_height_logical * scale;
    let padding = options.padding.unwrap_or(decoration.padding) * scale;
    let title_h = decoration.title_height * scale;
    let radius = decoration.corner_radius * scale;
    let gutter_gap = font_px * 1.15;

    let mut stats = Stats {
        lines: source.line_count(),
        language: String::new(),
        font_mono: fonts.mono.clone(),
        font_fallback: fonts.fallback.clone(),
        ..Stats::default()
    };

    // ---------------------------------------------------------------- 语法高亮
    let load_start = Instant::now();
    let highlighter =
        Highlighter::new(registry, palette, options.lang.as_deref(), source.path.as_deref())?;
    stats.syntax_load = load_start.elapsed();
    stats.language = highlighter.language.clone();
    let highlight_start = Instant::now();
    let spans = highlighter.highlight(&source.text, palette)?;
    stats.highlight = highlight_start.elapsed();

    let mono_family = fonts.mono.as_str();
    let fallback_family = fonts.fallback.as_str();
    let default_attrs = Attrs::new()
        .family(cosmic_text::Family::Name(mono_family))
        .color(palette.foreground.to_cosmic());

    // ---------------------------------------------------------------- 代码整形
    let shape_start = Instant::now();
    let metrics = Metrics::new(font_px, line_px);
    let mut buffer = Buffer::new(font_system, metrics);
    buffer.set_wrap(Wrap::None);
    buffer.set_size(None, None);

    let mut rich: Vec<(&str, Attrs)> = Vec::with_capacity(spans.len() + spans.len() / 4);
    for span in &spans {
        let piece = &source.text[span.start..span.end];
        let color = span.fg.to_cosmic();
        for (start, end, use_fallback) in split_runs(piece) {
            let family = if use_fallback {
                fallback_family
            } else {
                mono_family
            };
            let attrs = Attrs::new()
                .family(cosmic_text::Family::Name(family))
                .color(color)
                .weight(if span.bold {
                    Weight::BOLD
                } else {
                    Weight::NORMAL
                })
                .style(if span.italic {
                    FontStyle::Italic
                } else {
                    FontStyle::Normal
                });
            rich.push((&piece[start..end], attrs));
        }
    }
    buffer.set_rich_text(rich, &default_attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);

    let mut code_width: f32 = 0.0;
    let mut visual_lines = 0usize;
    for run in buffer.layout_runs() {
        visual_lines += 1;
        code_width = code_width.max(run.line_w);
    }
    stats.visual_lines = visual_lines;
    let code_height = visual_lines.max(1) as f32 * line_px;
    // 右侧留一点余量，避免斜体字形贴边；用比例值保证倍率严格线性。
    let code_width = code_width.max(font_px * 0.5) + font_px * 0.08;

    // ---------------------------------------------------------------- 几何布局
    // 行号：先量出空格宽度，再用「补空格右对齐」的方式一次整形整列行号。
    let (gutter_w, gutter_buffer) = if options.line_numbers {
        let mut probe = Buffer::new(font_system, metrics);
        probe.set_wrap(Wrap::None);
        probe.set_size(None, None);
        probe.set_text(" ", &default_attrs, Shaping::Advanced, None);
        probe.shape_until_scroll(font_system, false);
        let space_w = probe
            .layout_runs()
            .next()
            .map(|run| run.line_w)
            .unwrap_or(font_px * 0.6);
        let digits = decimal_digits(stats.lines.max(1));

        let mut text = String::with_capacity(stats.lines * (digits + 1));
        for number in 1..=stats.lines {
            use std::fmt::Write as _;
            if number > 1 {
                text.push('\n');
            }
            let _ = write!(text, "{number:>digits$}", digits = digits);
        }
        let attrs = Attrs::new()
            .family(cosmic_text::Family::Name(mono_family))
            .color(palette.gutter.to_cosmic());
        let mut gutter = Buffer::new(font_system, metrics);
        gutter.set_wrap(Wrap::None);
        gutter.set_size(None, None);
        gutter.set_text(&text, &attrs, Shaping::Advanced, None);
        gutter.shape_until_scroll(font_system, false);
        (digits as f32 * space_w, Some(gutter))
    } else {
        (0.0, None)
    };

    let gutter_block = if gutter_buffer.is_some() {
        gutter_w + gutter_gap
    } else {
        0.0
    };

    // 标题：用非等宽字体测量，必要时撑宽画布
    let title_size = font_px * decoration.title_font_scale;
    let title_attrs = Attrs::new()
        .family(cosmic_text::Family::Name(fallback_family))
        .color(palette.foreground.with_alpha(0.72).to_cosmic());
    let mut title_buffer = if title_h > 0.0 && !options.title.is_empty() {
        let mut buffer = Buffer::new(font_system, Metrics::new(title_size, title_h.max(title_size)));
        buffer.set_wrap(Wrap::None);
        buffer.set_size(None, None);
        buffer.set_text(&options.title, &title_attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(font_system, false);
        Some(buffer)
    } else {
        None
    };
    let title_width = title_buffer
        .as_ref()
        .map(|buffer| {
            buffer
                .layout_runs()
                .map(|run| run.line_w)
                .fold(0.0f32, f32::max)
        })
        .unwrap_or(0.0);

    let mut canvas_w = padding * 2.0 + gutter_block + code_width;
    let mut canvas_h = title_h + padding * 2.0 + code_height;
    if title_h > 0.0 {
        let min_w = match &decoration.controls {
            WindowControls::TrafficLights {
                spacing, inset, ..
            } => (inset + spacing * 3.0) * 2.0 * scale + title_width + 48.0 * scale,
            WindowControls::Windows { button_width, .. } => {
                title_width + button_width * 3.0 * scale + 64.0 * scale
            }
            WindowControls::None => title_width + 64.0 * scale,
        };
        canvas_w = canvas_w.max(min_w);
    }
    canvas_w = canvas_w.max(160.0 * scale);
    canvas_h = canvas_h.max(title_h + padding * 2.0 + line_px);
    stats.shape = shape_start.elapsed();

    // 阴影留白
    let margin = if options.shadow {
        (decoration.shadow.blur * 1.2 + decoration.shadow.offset_y + 8.0) * scale
    } else {
        0.0
    };
    // 先在「逻辑像素」上取整，再乘以倍率：这样 2x/3x 的宽高严格是 1x 的整数倍，
    // 不会因为 ceil 落点不同而出现 ±1px 的差异。
    let width = ((canvas_w + margin * 2.0) / scale).ceil().max(1.0) * scale;
    let height = ((canvas_h + margin * 2.0) / scale).ceil().max(1.0) * scale;
    let pixels = (width as u64) * (height as u64);
    if pixels > MAX_PIXELS {
        bail!(
            "输出图片过大：{}x{}（约 {:.1} 亿像素）。请降低 --scale 或缩小输入",
            width as u32,
            height as u32,
            pixels as f64 / 1e8
        );
    }
    let (width, height) = (width.round() as u32, height.round() as u32);
    stats.width = width;
    stats.height = height;

    // ---------------------------------------------------------------- 绘制
    let raster_start = Instant::now();
    let mut pixmap = Pixmap::new(width, height)
        .ok_or_else(|| anyhow::anyhow!("无法分配 {width}x{height} 的画布"))?;
    pixmap.fill(
        options
            .canvas
            .map(|color| color.to_skia())
            .unwrap_or(SkColor::TRANSPARENT),
    );

    let window = Rect::from_xywh(margin, margin, canvas_w, canvas_h)
        .ok_or_else(|| anyhow::anyhow!("画布尺寸无效"))?;
    let window_radius = radius.min(canvas_w.min(canvas_h) / 2.0);

    if options.shadow {
        paint_shadow(
            &mut pixmap,
            window,
            window_radius,
            decoration.shadow.offset_y * scale,
            decoration.shadow.blur * scale,
            decoration.shadow.alpha,
        );
    }

    // 窗口主体
    if let Some(path) = round_rect_path(window, [window_radius; 4]) {
        fill_path(&mut pixmap, &path, palette.background);
    }

    // 标题栏
    if title_h > 0.0 {
        let top_radius = window_radius.min(title_h);
        if let Some(titlebar) = Rect::from_xywh(margin, margin, canvas_w, title_h) {
            if let Some(path) = round_rect_path(titlebar, [top_radius, top_radius, 0.0, 0.0]) {
                fill_path(&mut pixmap, &path, decoration.titlebar_color(palette));
            }
        }
        // 分隔线
        if let Some(line) = Rect::from_xywh(margin, margin + title_h - scale, canvas_w, scale) {
            fill_rect(&mut pixmap, line, palette.foreground.with_alpha(0.07));
        }
    }

    let mut cache = SwashCache::new();

    // 标题栏内容
    if title_h > 0.0 {
        draw_title(
            &mut pixmap,
            font_system,
            &mut cache,
            palette,
            decoration,
            window,
            title_h,
            scale,
            title_buffer.as_mut(),
            &mut stats.glyphs,
        );
    }

    // 代码 + 行号
    let code_x = margin + padding + gutter_block;
    let code_y = margin + title_h + padding;
    if let Some(gutter) = &gutter_buffer {
        draw_buffer(
            &mut pixmap,
            font_system,
            &mut cache,
            gutter,
            (margin + padding, code_y),
            &mut stats.glyphs,
        );
    }
    draw_buffer(
        &mut pixmap,
        font_system,
        &mut cache,
        &buffer,
        (code_x, code_y),
        &mut stats.glyphs,
    );

    // 窗口描边（最后画，避免被内容覆盖）
    let border_alpha = if palette.is_light() { 0.95 } else { 0.9 };
    if let Some(path) = round_rect_path(
        Rect::from_xywh(
            window.left() + scale * 0.5,
            window.top() + scale * 0.5,
            (canvas_w - scale).max(1.0),
            (canvas_h - scale).max(1.0),
        )
        .unwrap_or(window),
        [(window_radius - scale * 0.5).max(0.0); 4],
    ) {
        stroke_path(
            &mut pixmap,
            &path,
            palette.border.with_alpha(border_alpha),
            scale,
        );
    }

    stats.raster = raster_start.elapsed();
    Ok(Rendered {
        pixmap,
        stats,
        syntax_origin: highlighter.origin.clone(),
    })
}

// ---------------------------------------------------------------------------
// 文本绘制
// ---------------------------------------------------------------------------

fn draw_buffer(
    pixmap: &mut Pixmap,
    font_system: &mut FontSystem,
    cache: &mut SwashCache,
    buffer: &Buffer,
    origin: (f32, f32),
    glyph_count: &mut usize,
) {
    let width = pixmap.width() as i32;
    let height = pixmap.height() as i32;
    let data = pixmap.data_mut();

    for run in buffer.layout_runs() {
        for glyph in run.glyphs {
            let physical = glyph.physical((origin.0, origin.1 + run.line_y), 1.0);
            let Some(image) = cache.get_image(font_system, physical.cache_key) else {
                continue;
            };
            *glyph_count += 1;
            let color = glyph
                .color_opt
                .map(|color| Rgba::new(color.r(), color.g(), color.b(), color.a()))
                .unwrap_or(Rgba::opaque(255, 255, 255));
            let x = physical.x + image.placement.left;
            let y = physical.y - image.placement.top;
            blit_image(
                data,
                width,
                height,
                &image.data,
                image.placement.width as i32,
                image.placement.height as i32,
                x,
                y,
                color,
                matches!(image.content, cosmic_text::SwashContent::Color),
            );
        }
    }
}

/// 把字形位图混合到 premultiplied RGBA 缓冲区。
#[allow(clippy::too_many_arguments)]
fn blit_image(
    dst: &mut [u8],
    dst_w: i32,
    dst_h: i32,
    src: &[u8],
    src_w: i32,
    src_h: i32,
    x0: i32,
    y0: i32,
    color: Rgba,
    is_color: bool,
) {
    let x_start = x0.max(0);
    let y_start = y0.max(0);
    let x_end = (x0 + src_w).min(dst_w);
    let y_end = (y0 + src_h).min(dst_h);
    if x_start >= x_end || y_start >= y_end {
        return;
    }

    let base_alpha = color.a as u32;
    // Mask（灰度覆盖）每像素 1 字节；Color（彩色 emoji）每像素 4 字节。
    let src_bpp = if is_color { 4 } else { 1 };
    for y in y_start..y_end {
        let mut src_index = ((y - y0) * src_w + (x_start - x0)) as usize * src_bpp;
        let mut dst_index = (y * dst_w + x_start) as usize * 4;
        for _ in x_start..x_end {
            if is_color {
                // 彩色字形（emoji）：源数据为 premultiplied RGBA
                let sa = src[src_index + 3] as u32;
                if sa != 0 {
                    let inv = 255 - sa;
                    dst[dst_index] =
                        (src[src_index] as u32 + dst[dst_index] as u32 * inv / 255) as u8;
                    dst[dst_index + 1] =
                        (src[src_index + 1] as u32 + dst[dst_index + 1] as u32 * inv / 255) as u8;
                    dst[dst_index + 2] =
                        (src[src_index + 2] as u32 + dst[dst_index + 2] as u32 * inv / 255) as u8;
                    dst[dst_index + 3] =
                        (sa + dst[dst_index + 3] as u32 * inv / 255).min(255) as u8;
                }
            } else {
                let coverage = src[src_index] as u32;
                if coverage != 0 {
                    let sa = coverage * base_alpha / 255;
                    if sa != 0 {
                        let inv = 255 - sa;
                        dst[dst_index] =
                            (color.r as u32 * sa / 255 + dst[dst_index] as u32 * inv / 255) as u8;
                        dst[dst_index + 1] = (color.g as u32 * sa / 255
                            + dst[dst_index + 1] as u32 * inv / 255)
                            as u8;
                        dst[dst_index + 2] = (color.b as u32 * sa / 255
                            + dst[dst_index + 2] as u32 * inv / 255)
                            as u8;
                        dst[dst_index + 3] =
                            (sa + dst[dst_index + 3] as u32 * inv / 255).min(255) as u8;
                    }
                }
            }
            src_index += src_bpp;
            dst_index += 4;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_title(
    pixmap: &mut Pixmap,
    font_system: &mut FontSystem,
    cache: &mut SwashCache,
    palette: &ResolvedPalette,
    decoration: &ResolvedDecoration,
    window: Rect,
    title_h: f32,
    scale: f32,
    title_buffer: Option<&mut Buffer>,
    glyph_count: &mut usize,
) {
    let controls_color = palette.foreground.with_alpha(0.62);

    match &decoration.controls {
        WindowControls::TrafficLights {
            radius,
            spacing,
            inset,
            close,
            minimize,
            maximize,
        } => {
            let r = radius * scale;
            let gap = spacing * scale;
            let left = window.left() + inset * scale;
            let cy = window.top() + title_h / 2.0;
            for (index, color) in [close, minimize, maximize].iter().enumerate() {
                let cx = left + r + index as f32 * gap;
                if let Some(path) = circle_path(cx, cy, r) {
                    let color = Rgba::from_hex(color).unwrap_or(Rgba::opaque(255, 255, 255));
                    fill_path(pixmap, &path, color);
                }
            }
        }
        WindowControls::Windows {
            icon_size,
            button_width,
            inset,
        } => {
            let size = icon_size * scale;
            let button = button_width * scale;
            let cy = window.top() + title_h / 2.0;
            let right = window.right() - inset * scale;
            let stroke = Stroke {
                width: scale.max(1.0),
                line_cap: LineCap::Round,
                line_join: LineJoin::Round,
                ..Stroke::default()
            };

            // 关闭
            let close_cx = right - button / 2.0;
            if let Some(path) = multi_line_path(&[
                (
                    close_cx - size / 2.0,
                    cy - size / 2.0,
                    close_cx + size / 2.0,
                    cy + size / 2.0,
                ),
                (
                    close_cx + size / 2.0,
                    cy - size / 2.0,
                    close_cx - size / 2.0,
                    cy + size / 2.0,
                ),
            ]) {
                stroke_path_with(pixmap, &path, controls_color, &stroke);
            }
            // 最大化
            let max_cx = right - button * 1.5;
            if let Some(rect) =
                Rect::from_xywh(max_cx - size / 2.0, cy - size / 2.0, size, size)
            {
                if let Some(path) = round_rect_path(rect, [0.0; 4]) {
                    stroke_path_with(pixmap, &path, controls_color, &stroke);
                }
            }
            // 最小化
            let min_cx = right - button * 2.5;
            if let Some(path) =
                multi_line_path(&[(min_cx - size / 2.0, cy, min_cx + size / 2.0, cy)])
            {
                stroke_path_with(pixmap, &path, controls_color, &stroke);
            }
        }
        WindowControls::None => {}
    }

    if let Some(buffer) = title_buffer {
        let title_width = buffer
            .layout_runs()
            .map(|run| run.line_w)
            .fold(0.0f32, f32::max);
        let x = match decoration.title_align {
            TitleAlign::Center => window.left() + (window.width() - title_width) / 2.0,
            TitleAlign::Left => window.left() + 16.0 * scale,
        };
        draw_buffer(
            pixmap,
            font_system,
            cache,
            buffer,
            (x, window.top()),
            glyph_count,
        );
    }
}

// ---------------------------------------------------------------------------
// 矢量绘制辅助
// ---------------------------------------------------------------------------

fn fill_rect(pixmap: &mut Pixmap, rect: Rect, color: Rgba) {
    if let Some(path) = round_rect_path(rect, [0.0; 4]) {
        fill_path(pixmap, &path, color);
    }
}

fn fill_path(pixmap: &mut Pixmap, path: &Path, color: Rgba) {
    let mut paint = Paint::default();
    paint.set_color(color.to_skia());
    paint.anti_alias = true;
    pixmap.fill_path(path, &paint, FillRule::Winding, Transform::identity(), None);
}

fn stroke_path(pixmap: &mut Pixmap, path: &Path, color: Rgba, width: f32) {
    let stroke = Stroke {
        width: width.max(1.0),
        ..Stroke::default()
    };
    stroke_path_with(pixmap, path, color, &stroke);
}

fn stroke_path_with(pixmap: &mut Pixmap, path: &Path, color: Rgba, stroke: &Stroke) {
    let mut paint = Paint::default();
    paint.set_color(color.to_skia());
    paint.anti_alias = true;
    pixmap.stroke_path(path, &paint, stroke, Transform::identity(), None);
}

/// 四角可独立指定半径的圆角矩形，用三次贝塞尔（kappa ≈ 0.5523）逼近真圆弧。
fn round_rect_path(rect: Rect, radius: [f32; 4]) -> Option<Path> {
    const KAPPA: f32 = 0.552_284_8;
    let max_radius = rect.width().min(rect.height()) / 2.0;
    let [tl, tr, br, bl] = radius.map(|r| r.clamp(0.0, max_radius));
    let (x, y, w, h) = (rect.left(), rect.top(), rect.width(), rect.height());
    let mut builder = PathBuilder::new();
    builder.move_to(x + tl, y);
    builder.line_to(x + w - tr, y);
    if tr > 0.0 {
        builder.cubic_to(
            x + w - tr + tr * KAPPA,
            y,
            x + w,
            y + tr - tr * KAPPA,
            x + w,
            y + tr,
        );
    }
    builder.line_to(x + w, y + h - br);
    if br > 0.0 {
        builder.cubic_to(
            x + w,
            y + h - br + br * KAPPA,
            x + w - br + br * KAPPA,
            y + h,
            x + w - br,
            y + h,
        );
    }
    builder.line_to(x + bl, y + h);
    if bl > 0.0 {
        builder.cubic_to(
            x + bl - bl * KAPPA,
            y + h,
            x,
            y + h - bl + bl * KAPPA,
            x,
            y + h - bl,
        );
    }
    builder.line_to(x, y + tl);
    if tl > 0.0 {
        builder.cubic_to(
            x,
            y + tl - tl * KAPPA,
            x + tl - tl * KAPPA,
            y,
            x + tl,
            y,
        );
    }
    builder.close();
    builder.finish()
}

fn circle_path(cx: f32, cy: f32, r: f32) -> Option<Path> {
    if r <= 0.0 {
        return None;
    }
    let mut builder = PathBuilder::new();
    builder.push_circle(cx, cy, r);
    builder.finish()
}

fn multi_line_path(points: &[(f32, f32, f32, f32)]) -> Option<Path> {
    let mut builder = PathBuilder::new();
    for (x0, y0, x1, y1) in points {
        builder.move_to(*x0, *y0);
        builder.line_to(*x1, *y1);
    }
    builder.finish()
}

/// 低分辨率模糊阴影：SDF 覆盖 → 3 次盒式模糊 → 双线性上采样，
/// 只合成窗口外侧的四个条带，避免对大图做全图逐像素处理。
fn paint_shadow(
    pixmap: &mut Pixmap,
    window: Rect,
    radius: f32,
    offset_y: f32,
    blur: f32,
    alpha: f32,
) {
    if blur <= 0.0 || alpha <= 0.0 {
        return;
    }
    let pad = blur * 1.6 + offset_y.abs() + 4.0;
    let region = Rect::from_ltrb(
        window.left() - pad,
        window.top() - pad,
        window.right() + pad,
        window.bottom() + pad,
    )
    .unwrap_or(window);

    // 大图用更低的阴影分辨率：阴影本身是低频的，8x 降采样肉眼无损，
    // 但能把遮罩内存/模糊开销再降到 1/4。
    let area = pixmap.width() as u64 * pixmap.height() as u64;
    let downsample = if area > 50_000_000 { 8.0f32 } else { 4.0 };
    let mw = ((region.width() / downsample).ceil() as usize + 2).max(2);
    let mh = ((region.height() / downsample).ceil() as usize + 2).max(2);
    let mut mask = vec![0f32; mw * mh];

    for j in 0..mh {
        let py = region.top() + (j as f32 + 0.5) * downsample - offset_y;
        for i in 0..mw {
            let px = region.left() + (i as f32 + 0.5) * downsample;
            let distance = rounded_box_distance(px, py, window, radius);
            mask[j * mw + i] = (0.5 - distance / downsample).clamp(0.0, 1.0);
        }
    }

    let radius_px = ((blur / downsample) * 0.55).round().max(1.0) as usize;
    let mut scratch = vec![0f32; mw * mh];
    for _ in 0..3 {
        box_blur(&mut mask, &mut scratch, mw, mh, radius_px);
    }

    let pixmap_w = pixmap.width() as i32;
    let pixmap_h = pixmap.height() as i32;
    let color = Rgba::opaque(0, 0, 0);
    let data = pixmap.data_mut();

    let mut composite = |x0: f32, y0: f32, x1: f32, y1: f32| {
        let x0 = x0.max(0.0).floor() as i32;
        let y0 = y0.max(0.0).floor() as i32;
        let x1 = x1.min(pixmap_w as f32).ceil() as i32;
        let y1 = y1.min(pixmap_h as f32).ceil() as i32;
        for y in y0..y1 {
            for x in x0..x1 {
                let u = (x as f32 + 0.5 - region.left()) / downsample - 0.5;
                let v = (y as f32 + 0.5 - region.top()) / downsample - 0.5;
                let a = sample_bilinear(&mask, mw, mh, u, v) * alpha;
                if a <= 0.003 {
                    continue;
                }
                let index = ((y * pixmap_w + x) * 4) as usize;
                blend_over(data, index, color, a);
            }
        }
    };

    // 上 / 下
    composite(region.left(), region.top(), region.right(), window.top());
    composite(
        region.left(),
        window.bottom(),
        region.right(),
        region.bottom(),
    );
    // 左 / 右（避开上下条带，重复混合会加深）
    composite(region.left(), window.top(), window.left(), window.bottom());
    composite(
        window.right(),
        window.top(),
        region.right(),
        window.bottom(),
    );
}

fn rounded_box_distance(px: f32, py: f32, rect: Rect, radius: f32) -> f32 {
    let cx = rect.left() + rect.width() / 2.0;
    let cy = rect.top() + rect.height() / 2.0;
    let qx = (px - cx).abs() - (rect.width() / 2.0 - radius);
    let qy = (py - cy).abs() - (rect.height() / 2.0 - radius);
    let ax = qx.max(0.0);
    let ay = qy.max(0.0);
    (ax * ax + ay * ay).sqrt() + qx.max(qy).min(0.0) - radius
}

/// 可分离盒式模糊（边界按边缘像素延拓），用滑动窗口保证 O(w*h)。
fn box_blur(data: &mut [f32], scratch: &mut [f32], width: usize, height: usize, radius: usize) {
    if radius == 0 || width < 2 || height < 2 {
        return;
    }
    let norm = 1.0 / (radius * 2 + 1) as f32;
    let clamp = |value: isize, max: usize| -> usize {
        value.clamp(0, max as isize - 1) as usize
    };

    // 水平：data -> scratch
    for y in 0..height {
        let row = y * width;
        let mut sum = 0.0f32;
        for offset in -(radius as isize)..=(radius as isize) {
            sum += data[row + clamp(offset, width)];
        }
        for x in 0..width {
            scratch[row + x] = sum * norm;
            let add = data[row + clamp(x as isize + radius as isize + 1, width)];
            let remove = data[row + clamp(x as isize - radius as isize, width)];
            sum += add - remove;
        }
    }

    // 垂直：scratch -> data
    for x in 0..width {
        let mut sum = 0.0f32;
        for offset in -(radius as isize)..=(radius as isize) {
            sum += scratch[clamp(offset, height) * width + x];
        }
        for y in 0..height {
            data[y * width + x] = sum * norm;
            let add = scratch[clamp(y as isize + radius as isize + 1, height) * width + x];
            let remove = scratch[clamp(y as isize - radius as isize, height) * width + x];
            sum += add - remove;
        }
    }
}

fn sample_bilinear(data: &[f32], width: usize, height: usize, u: f32, v: f32) -> f32 {
    let u = u.clamp(0.0, width as f32 - 1.0);
    let v = v.clamp(0.0, height as f32 - 1.0);
    let x0 = u.floor() as usize;
    let y0 = v.floor() as usize;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let fx = u - x0 as f32;
    let fy = v - y0 as f32;
    let top = data[y0 * width + x0] * (1.0 - fx) + data[y0 * width + x1] * fx;
    let bottom = data[y1 * width + x0] * (1.0 - fx) + data[y1 * width + x1] * fx;
    top * (1.0 - fy) + bottom * fy
}

#[inline]
fn blend_over(data: &mut [u8], index: usize, color: Rgba, coverage: f32) {
    let sa = (coverage.clamp(0.0, 1.0) * color.a as f32).round() as u32;
    if sa == 0 {
        return;
    }
    let inv = 255 - sa;
    data[index] = (color.r as u32 * sa / 255 + data[index] as u32 * inv / 255) as u8;
    data[index + 1] = (color.g as u32 * sa / 255 + data[index + 1] as u32 * inv / 255) as u8;
    data[index + 2] = (color.b as u32 * sa / 255 + data[index + 2] as u32 * inv / 255) as u8;
    data[index + 3] = (sa + data[index + 3] as u32 * inv / 255).min(255) as u8;
}

// ---------------------------------------------------------------------------
// 杂项
// ---------------------------------------------------------------------------

fn decimal_digits(mut value: usize) -> usize {
    let mut digits = 1;
    while value >= 10 {
        value /= 10;
        digits += 1;
    }
    digits
}

/// 把一段文本按「等宽字体可覆盖 / 需要用非等宽回退字体」切分。
fn split_runs(text: &str) -> Vec<(usize, usize, bool)> {
    let mut runs = Vec::new();
    let mut start = 0usize;
    let mut current: Option<bool> = None;
    for (index, ch) in text.char_indices() {
        let fallback = needs_fallback(ch);
        match current {
            None => {
                current = Some(fallback);
                start = index;
            }
            Some(value) if value != fallback => {
                runs.push((start, index, value));
                start = index;
                current = Some(fallback);
            }
            _ => {}
        }
    }
    if let Some(value) = current {
        runs.push((start, text.len(), value));
    }
    runs
}

/// CJK / 全角标点 / 韩文 / 假名 / emoji：JetBrains Mono 不覆盖，交给系统非等宽字体。
fn needs_fallback(ch: char) -> bool {
    let c = ch as u32;
    matches!(c,
        0x1100..=0x11FF
        | 0x2600..=0x27BF
        | 0x2E80..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xAC00..=0xD7AF
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFFEF
        | 0x1F000..=0x1FAFF
        | 0x20000..=0x3FFFF
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_cjk_runs() {
        let runs = split_runs("let s = \"你好\";");
        assert_eq!(runs.len(), 3);
        assert!(!runs[0].2);
        assert!(runs[1].2);
        assert!(!runs[2].2);
    }

    #[test]
    fn digits_count() {
        assert_eq!(decimal_digits(1), 1);
        assert_eq!(decimal_digits(9), 1);
        assert_eq!(decimal_digits(10), 2);
        assert_eq!(decimal_digits(12345), 5);
    }

    #[test]
    fn distance_inside_is_negative() {
        let rect = Rect::from_xywh(10.0, 10.0, 100.0, 50.0).unwrap();
        assert!(rounded_box_distance(60.0, 35.0, rect, 8.0) < 0.0);
        assert!(rounded_box_distance(0.0, 0.0, rect, 8.0) > 0.0);
    }

    #[test]
    fn box_blur_preserves_constant_field() {
        let (w, h) = (9, 7);
        let mut data = vec![1.0f32; w * h];
        let mut scratch = vec![0.0f32; w * h];
        box_blur(&mut data, &mut scratch, w, h, 3);
        for value in data {
            assert!((value - 1.0).abs() < 1e-4);
        }
    }
}
