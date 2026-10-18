//! 渲染管线的集成测试。

use coderender::color::Rgba;
use coderender::document::Source;
use coderender::fonts::FontSet;
use coderender::render::{render, RenderOptions};
use coderender::syntax::SyntaxRegistry;
use coderender::theme::{ThemePack, ThemeRegistry};

fn source(text: &str) -> Source {
    Source {
        text: text.to_string(),
        path: None,
        display_name: "test.rs".to_string(),
        lossy: false,
    }
}

fn options(scale: f32) -> RenderOptions {
    RenderOptions {
        scale,
        font_size: 14.0,
        line_height: 1.55,
        padding: None,
        line_numbers: true,
        shadow: false,
        canvas: None,
        title: "test.rs".to_string(),
        lang: Some("rust".to_string()),
    }
}

fn color_at(pixmap: &tiny_skia::Pixmap, x: u32, y: u32) -> Rgba {
    let index = ((y * pixmap.width() + x) * 4) as usize;
    let data = pixmap.data();
    Rgba::new(
        data[index],
        data[index + 1],
        data[index + 2],
        data[index + 3],
    )
}

#[test]
fn renders_default_palette_and_decoration() {
    let themes = ThemeRegistry::load(None).unwrap();
    let palette = themes.palette("mocha").unwrap();
    let decoration = themes.decoration("macos").unwrap();
    let mut system = coderender::fonts::font_system();
    let fonts = FontSet::resolve(&system, None, None).unwrap();
    let registry = SyntaxRegistry::load(None, false).unwrap();

    let text = "fn main() {\n    println!(\"hi\");\n}";
    let rendered = render(
        &source(text),
        &palette,
        &decoration,
        &fonts,
        &mut system,
        &options(1.0),
        &registry,
    )
    .unwrap();

    let stats = &rendered.stats;
    assert_eq!(stats.lines, 3);
    assert!(stats.glyphs > 20);
    assert_eq!(stats.width, rendered.pixmap.width());

    // 圆角外应当保持透明
    assert_eq!(color_at(&rendered.pixmap, 0, 0).a, 0);
    // 标题栏（避开红绿灯与标题文字）
    let titlebar = color_at(&rendered.pixmap, stats.width - 8, 6);
    assert_eq!(titlebar, palette.titlebar);
    // 代码区背景
    let body = color_at(&rendered.pixmap, stats.width - 8, stats.height - 8);
    assert_eq!(body, palette.background);
}

#[test]
fn higher_scale_is_exactly_vector_scaled() {
    let themes = ThemeRegistry::load(None).unwrap();
    let palette = themes.palette("latte").unwrap();
    let decoration = themes.decoration("windows").unwrap();
    let mut system = coderender::fonts::font_system();
    let fonts = FontSet::resolve(&system, None, None).unwrap();
    let registry = SyntaxRegistry::load(None, false).unwrap();

    let text = "// 你好，世界\nlet answer: u64 = 42;\n";
    let one = render(
        &source(text),
        &palette,
        &decoration,
        &fonts,
        &mut system,
        &options(1.0),
        &registry,
    )
    .unwrap();
    let two = render(
        &source(text),
        &palette,
        &decoration,
        &fonts,
        &mut system,
        &options(2.0),
        &registry,
    )
    .unwrap();

    assert_eq!(two.pixmap.width(), one.pixmap.width() * 2);
    assert_eq!(two.pixmap.height(), one.pixmap.height() * 2);
    assert_eq!(two.stats.visual_lines, one.stats.visual_lines);
}

#[test]
fn line_numbers_widen_the_image() {
    let themes = ThemeRegistry::load(None).unwrap();
    let palette = themes.palette("mocha").unwrap();
    let decoration = themes.decoration("none").unwrap();
    let mut system = coderender::fonts::font_system();
    let fonts = FontSet::resolve(&system, None, None).unwrap();
    let registry = SyntaxRegistry::load(None, false).unwrap();

    let mut with_numbers = options(1.0);
    let mut without_numbers = options(1.0);
    without_numbers.line_numbers = false;

    let text = "let answer: u64 = 42; // 这一行足够长，不会被最小宽度兜底\nfn main() {}";
    let a = render(
        &source(text),
        &palette,
        &decoration,
        &fonts,
        &mut system,
        &with_numbers,
        &registry,
    )
    .unwrap();
    let b = render(
        &source(text),
        &palette,
        &decoration,
        &fonts,
        &mut system,
        &without_numbers,
        &registry,
    )
    .unwrap();
    assert!(a.pixmap.width() > b.pixmap.width());

    with_numbers.scale = 3.0;
    let huge = render(
        &source(text),
        &palette,
        &decoration,
        &fonts,
        &mut system,
        &with_numbers,
        &registry,
    )
    .unwrap();
    assert_eq!(huge.pixmap.width(), a.pixmap.width() * 3);
}

#[test]
fn empty_and_plain_text_are_safe() {
    let themes = ThemeRegistry::load(None).unwrap();
    let palette = themes.palette("frappe").unwrap();
    let decoration = themes.decoration("none").unwrap();
    let mut system = coderender::fonts::font_system();
    let fonts = FontSet::resolve(&system, None, None).unwrap();
    let registry = SyntaxRegistry::load(None, false).unwrap();

    let mut plain = options(1.0);
    plain.lang = Some("txt".to_string());
    for text in ["", "\n", "只有一行中文", "no newline at end"] {
        let rendered = render(
            &source(text),
            &palette,
            &decoration,
            &fonts,
            &mut system,
            &plain,
            &registry,
        )
        .unwrap();
        assert!(rendered.pixmap.width() > 0);
        assert!(rendered.pixmap.height() > 0);
    }
}

#[test]
fn loads_a_minimal_custom_pack() {
    let pack_text = r##"
[pack]
id = "mini"

[palettes.plain]
background = "#000000"
foreground = "#ffffff"
titlebar = "#111111"
border = "#222222"

[palettes.plain.syntax]
keyword = "#ff0000"

[decorations.bare]
title_height = 0
corner_radius = 0

[decorations.bare.controls]
style = "none"
"##;

    let pack = ThemePack::from_toml(pack_text, "mini.toml").unwrap();
    let palette = pack.palette("PLAIN").unwrap();
    assert_eq!(palette.token(coderender::theme::TokenKind::Keyword), Rgba::opaque(255, 0, 0));
    assert_eq!(palette.token(coderender::theme::TokenKind::String), palette.foreground);
    let decoration = pack.decoration("bare").unwrap();
    assert_eq!(decoration.title_height, 0.0);
    assert_eq!(decoration.padding, 28.0);
}
