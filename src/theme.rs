//! 主题包：代码配色预设（[palettes]）+ 窗口装饰预设（[decorations]）。
//!
//! 主题包是纯 TOML 文件，内置包编译进二进制（`assets/themes/catppuccin.toml`），
//! 也可以用 `--theme-pack` 加载自定义包。

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;

use crate::color::Rgba;

pub const BUILTIN_PACK: &str = include_str!("../assets/themes/catppuccin.toml");
pub const DEFAULT_PALETTE: &str = "mocha";
pub const DEFAULT_DECORATION: &str = "macos";
const DEFAULT_PADDING: f32 = 28.0;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemePack {
    pub pack: PackMeta,
    #[serde(default)]
    pub palettes: BTreeMap<String, Palette>,
    #[serde(default)]
    pub decorations: BTreeMap<String, Decoration>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackMeta {
    pub id: String,
    /// `theme` / `syntax`；安装时据此把包放进对应的目录。
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    #[default]
    Dark,
    Light,
}

impl Appearance {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }
}

/// 所有语法着色类别。未在主题包中显式给出的类别会从 foreground 派生。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Keyword,
    Function,
    Type,
    Builtin,
    String,
    Number,
    Constant,
    Comment,
    Operator,
    Punctuation,
    Variable,
    Parameter,
    Property,
    Attribute,
    Tag,
    Escape,
    Invalid,
}

impl TokenKind {
    pub const COUNT: usize = 17;
    pub const ALL: [TokenKind; Self::COUNT] = [
        TokenKind::Keyword,
        TokenKind::Function,
        TokenKind::Type,
        TokenKind::Builtin,
        TokenKind::String,
        TokenKind::Number,
        TokenKind::Constant,
        TokenKind::Comment,
        TokenKind::Operator,
        TokenKind::Punctuation,
        TokenKind::Variable,
        TokenKind::Parameter,
        TokenKind::Property,
        TokenKind::Attribute,
        TokenKind::Tag,
        TokenKind::Escape,
        TokenKind::Invalid,
    ];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|k| *k == self).expect("TokenKind::ALL 完整")
    }

    /// 与 TOML 中的字段名一致，也用于 `themes` 输出。
    pub fn key(self) -> &'static str {
        match self {
            TokenKind::Keyword => "keyword",
            TokenKind::Function => "function",
            TokenKind::Type => "type",
            TokenKind::Builtin => "builtin",
            TokenKind::String => "string",
            TokenKind::Number => "number",
            TokenKind::Constant => "constant",
            TokenKind::Comment => "comment",
            TokenKind::Operator => "operator",
            TokenKind::Punctuation => "punctuation",
            TokenKind::Variable => "variable",
            TokenKind::Parameter => "parameter",
            TokenKind::Property => "property",
            TokenKind::Attribute => "attribute",
            TokenKind::Tag => "tag",
            TokenKind::Escape => "escape",
            TokenKind::Invalid => "invalid",
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyntaxColors {
    pub keyword: Option<String>,
    pub function: Option<String>,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub builtin: Option<String>,
    pub string: Option<String>,
    pub number: Option<String>,
    pub constant: Option<String>,
    pub comment: Option<String>,
    pub operator: Option<String>,
    pub punctuation: Option<String>,
    pub variable: Option<String>,
    pub parameter: Option<String>,
    pub property: Option<String>,
    pub attribute: Option<String>,
    pub tag: Option<String>,
    pub escape: Option<String>,
    pub invalid: Option<String>,
}

impl SyntaxColors {
    pub fn get(&self, kind: TokenKind) -> Option<&str> {
        let slot = match kind {
            TokenKind::Keyword => &self.keyword,
            TokenKind::Function => &self.function,
            TokenKind::Type => &self.type_,
            TokenKind::Builtin => &self.builtin,
            TokenKind::String => &self.string,
            TokenKind::Number => &self.number,
            TokenKind::Constant => &self.constant,
            TokenKind::Comment => &self.comment,
            TokenKind::Operator => &self.operator,
            TokenKind::Punctuation => &self.punctuation,
            TokenKind::Variable => &self.variable,
            TokenKind::Parameter => &self.parameter,
            TokenKind::Property => &self.property,
            TokenKind::Attribute => &self.attribute,
            TokenKind::Tag => &self.tag,
            TokenKind::Escape => &self.escape,
            TokenKind::Invalid => &self.invalid,
        };
        slot.as_deref()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Palette {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub appearance: Appearance,
    pub background: String,
    #[serde(default)]
    pub titlebar: Option<String>,
    #[serde(default)]
    pub border: Option<String>,
    pub foreground: String,
    #[serde(default)]
    pub gutter: Option<String>,
    #[serde(default)]
    pub syntax: SyntaxColors,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TitleAlign {
    #[default]
    Left,
    Center,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TitlebarStyle {
    /// 使用 palette.titlebar（缺省时从背景色微微提亮/压暗）
    #[default]
    Solid,
    /// 由背景色向文字色混合 4%
    Tint,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShadowSpec {
    #[serde(default = "default_shadow_offset")]
    pub offset_y: f32,
    #[serde(default = "default_shadow_blur")]
    pub blur: f32,
    #[serde(default = "default_shadow_alpha")]
    pub alpha: f32,
}

fn default_shadow_offset() -> f32 {
    10.0
}
fn default_shadow_blur() -> f32 {
    26.0
}
fn default_shadow_alpha() -> f32 {
    0.35
}

impl Default for ShadowSpec {
    fn default() -> Self {
        Self {
            offset_y: default_shadow_offset(),
            blur: default_shadow_blur(),
            alpha: default_shadow_alpha(),
        }
    }
}

/// 窗口按钮装饰。内部 tag = "style"。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(tag = "style", rename_all = "snake_case")]
pub enum WindowControls {
    /// macOS 红绿灯
    TrafficLights {
        #[serde(default = "default_light_radius")]
        radius: f32,
        #[serde(default = "default_light_spacing")]
        spacing: f32,
        #[serde(default = "default_light_inset")]
        inset: f32,
        #[serde(default = "default_close_color")]
        close: String,
        #[serde(default = "default_minimize_color")]
        minimize: String,
        #[serde(default = "default_maximize_color")]
        maximize: String,
    },
    /// Windows 11 最小化 / 最大化 / 关闭
    Windows {
        #[serde(default = "default_icon_size")]
        icon_size: f32,
        #[serde(default = "default_button_width")]
        button_width: f32,
        #[serde(default = "default_windows_inset")]
        inset: f32,
    },
    /// 不绘制窗口按钮
    #[default]
    None,
}

fn default_light_radius() -> f32 {
    6.5
}
fn default_light_spacing() -> f32 {
    19.0
}
fn default_light_inset() -> f32 {
    20.0
}
fn default_close_color() -> String {
    "#ff5f57".into()
}
fn default_minimize_color() -> String {
    "#febc2e".into()
}
fn default_maximize_color() -> String {
    "#28c840".into()
}
fn default_icon_size() -> f32 {
    10.0
}
fn default_button_width() -> f32 {
    46.0
}
fn default_windows_inset() -> f32 {
    8.0
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decoration {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub title_height: f32,
    #[serde(default = "default_corner_radius")]
    pub corner_radius: f32,
    #[serde(default)]
    pub title_align: TitleAlign,
    #[serde(default)]
    pub titlebar_style: TitlebarStyle,
    #[serde(default = "default_title_font_scale")]
    pub title_font_scale: f32,
    #[serde(default)]
    pub padding: Option<f32>,
    #[serde(default)]
    pub shadow: Option<ShadowSpec>,
    #[serde(default)]
    pub controls: WindowControls,
}

fn default_corner_radius() -> f32 {
    12.0
}
fn default_title_font_scale() -> f32 {
    0.82
}

/// 解析完成、可直接用于渲染的配色。
#[derive(Debug, Clone)]
pub struct ResolvedPalette {
    pub id: String,
    pub name: String,
    pub appearance: Appearance,
    pub background: Rgba,
    pub titlebar: Rgba,
    pub border: Rgba,
    pub foreground: Rgba,
    pub gutter: Rgba,
    pub syntax: [Rgba; TokenKind::COUNT],
}

impl ResolvedPalette {
    pub fn token(&self, kind: TokenKind) -> Rgba {
        self.syntax[kind.index()]
    }

    pub fn is_light(&self) -> bool {
        self.appearance == Appearance::Light
    }
}

/// 解析完成的窗口装饰。
#[derive(Debug, Clone)]
pub struct ResolvedDecoration {
    pub id: String,
    pub name: String,
    pub title_height: f32,
    pub corner_radius: f32,
    pub title_align: TitleAlign,
    pub titlebar_style: TitlebarStyle,
    pub title_font_scale: f32,
    pub padding: f32,
    pub shadow: ShadowSpec,
    pub controls: WindowControls,
}

impl ResolvedDecoration {
    pub fn titlebar_color(&self, palette: &ResolvedPalette) -> Rgba {
        match self.titlebar_style {
            TitlebarStyle::Solid => palette.titlebar,
            TitlebarStyle::Tint => palette.background.mix(palette.foreground, 0.045),
        }
    }
}

impl ThemePack {
    pub fn builtin() -> Result<Self> {
        Self::from_toml(BUILTIN_PACK, "<builtin catppuccin>")
    }

    pub fn load(path: Option<&Path>) -> Result<Self> {
        match path {
            None => Self::builtin(),
            Some(path) => {
                let text = std::fs::read_to_string(path)
                    .with_context(|| format!("无法读取主题包 {}", path.display()))?;
                Self::from_toml(&text, &path.display().to_string())
            }
        }
    }

    pub fn from_toml(text: &str, origin: &str) -> Result<Self> {
        let pack: Self = toml::from_str(text)
            .with_context(|| format!("主题包 {origin} 解析失败（请检查字段名与颜色格式）"))?;
        pack.validate(origin)?;
        Ok(pack)
    }

    fn validate(&self, origin: &str) -> Result<()> {
        if self.pack.id.trim().is_empty() {
            bail!("主题包 {origin} 缺少 pack.id");
        }
        if self.palettes.is_empty() {
            bail!("主题包 {origin} 没有任何 [palettes.*] 配色预设");
        }
        if self.decorations.is_empty() {
            bail!("主题包 {origin} 没有任何 [decorations.*] 窗口装饰预设");
        }
        for id in self.palettes.keys() {
            self.palette(id)
                .with_context(|| format!("主题包 {origin} 的配色预设 `{id}` 无效"))?;
        }
        for (id, decoration) in &self.decorations {
            if decoration.title_height < 0.0 {
                bail!("窗口装饰 `{id}` 的 title_height 不能为负数");
            }
            if decoration.corner_radius < 0.0 {
                bail!("窗口装饰 `{id}` 的 corner_radius 不能为负数");
            }
            if let Some(padding) = decoration.padding {
                if padding < 0.0 {
                    bail!("窗口装饰 `{id}` 的 padding 不能为负数");
                }
            }
            if let WindowControls::TrafficLights {
                close,
                minimize,
                maximize,
                ..
            } = &decoration.controls
            {
                Rgba::from_hex(close)
                    .with_context(|| format!("窗口装饰 `{id}` 的红绿灯颜色 close 无效"))?;
                Rgba::from_hex(minimize)
                    .with_context(|| format!("窗口装饰 `{id}` 的红绿灯颜色 minimize 无效"))?;
                Rgba::from_hex(maximize)
                    .with_context(|| format!("窗口装饰 `{id}` 的红绿灯颜色 maximize 无效"))?;
            }
        }
        Ok(())
    }

    pub fn palette(&self, id: &str) -> Result<ResolvedPalette> {
        let (key, raw) = self
            .find_key(&self.palettes, id)
            .ok_or_else(|| {
                self.unknown(
                    id,
                    "配色预设",
                    self.palettes.keys(),
                    self.palettes.values().filter_map(|p| p.name.clone()),
                )
            })?;

        let background = parse_color(&raw.background, &key, "background")?;
        let foreground = parse_color(&raw.foreground, &key, "foreground")?;
        let titlebar = raw
            .titlebar
            .as_deref()
            .map(|s| parse_color(s, &key, "titlebar"))
            .transpose()?
            .unwrap_or_else(|| default_titlebar(background, foreground, raw.appearance));
        let border = raw
            .border
            .as_deref()
            .map(|s| parse_color(s, &key, "border"))
            .transpose()?
            .unwrap_or_else(|| background.mix(foreground, 0.14));
        let gutter = raw
            .gutter
            .as_deref()
            .map(|s| parse_color(s, &key, "gutter"))
            .transpose()?
            .unwrap_or_else(|| foreground.mix(background, 0.55));

        let mut syntax = [foreground; TokenKind::COUNT];
        for kind in TokenKind::ALL {
            let default = match kind {
                TokenKind::Comment => foreground.mix(background, 0.42),
                TokenKind::Punctuation => foreground.mix(background, 0.3),
                _ => foreground,
            };
            syntax[kind.index()] = match raw.syntax.get(kind) {
                Some(value) => parse_color(value, &key, kind.key())?,
                None => default,
            };
        }

        Ok(ResolvedPalette {
            id: key,
            name: raw.name.clone().unwrap_or_else(|| id.to_string()),
            appearance: raw.appearance,
            background,
            titlebar,
            border,
            foreground,
            gutter,
            syntax,
        })
    }

    pub fn decoration(&self, id: &str) -> Result<ResolvedDecoration> {
        let (key, raw) = self
            .find_key(&self.decorations, id)
            .ok_or_else(|| {
                self.unknown(
                    id,
                    "窗口装饰预设",
                    self.decorations.keys(),
                    self.decorations.values().filter_map(|d| d.name.clone()),
                )
            })?;
        Ok(ResolvedDecoration {
            name: raw.name.clone().unwrap_or_else(|| key.clone()),
            id: key,
            title_height: raw.title_height,
            corner_radius: raw.corner_radius,
            title_align: raw.title_align,
            titlebar_style: raw.titlebar_style,
            title_font_scale: raw.title_font_scale,
            padding: raw.padding.unwrap_or(DEFAULT_PADDING),
            shadow: raw.shadow.clone().unwrap_or_default(),
            controls: raw.controls.clone(),
        })
    }

    /// 先按 id 精确/归一化匹配，再按显示名匹配。
    fn find_key<'a, V>(&self, map: &'a BTreeMap<String, V>, id: &str) -> Option<(String, &'a V)>
    where
        V: HasName,
    {
        let wanted = fold_key(id);
        if let Some((key, value)) = map.iter().find(|(key, _)| fold_key(key) == wanted) {
            return Some((key.clone(), value));
        }
        map.iter()
            .find(|(_, value)| {
                value
                    .name()
                    .map(|name| fold_key(name) == wanted)
                    .unwrap_or(false)
            })
            .map(|(key, value)| (key.clone(), value))
    }

    fn unknown<'a>(
        &self,
        id: &str,
        what: &str,
        keys: impl Iterator<Item = &'a String>,
        names: impl IntoIterator<Item = String>,
    ) -> anyhow::Error {
        let available: Vec<&str> = keys.map(String::as_str).collect();
        let mut candidates = available.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        candidates.extend(names);
        candidates.sort_by_key(|name| name.to_lowercase());
        candidates.dedup();
        anyhow!(
            "主题包中没有{what} `{id}`{}。可用 `coderender themes` 查看所有预设。",
            crate::dym::did_you_mean(id, &candidates, 4)
        )
    }

    pub fn palette_ids(&self) -> Vec<&str> {
        self.palettes.keys().map(String::as_str).collect()
    }

    pub fn decoration_ids(&self) -> Vec<&str> {
        self.decorations.keys().map(String::as_str).collect()
    }
}

trait HasName {
    fn name(&self) -> Option<&str>;
}

impl HasName for Palette {
    fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
}

impl HasName for Decoration {
    fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
}

fn parse_color(value: &str, palette_id: &str, field: &str) -> Result<Rgba> {
    Rgba::from_hex(value)
        .with_context(|| format!("配色预设 `{palette_id}` 的 {field} = `{value}` 无法解析"))
}

fn default_titlebar(background: Rgba, foreground: Rgba, appearance: Appearance) -> Rgba {
    match appearance {
        Appearance::Dark => background.mix(foreground, 0.035),
        Appearance::Light => background.mix(foreground, 0.02),
    }
}

/// 主题包注册表：内置包 + 用户已安装的主题包 + `--theme-pack` 指定的包。
///
/// `--theme` / `--window` 会依次在这些包里查找，因此安装新主题包后
/// 无需任何额外参数即可直接使用。
pub struct ThemeRegistry {
    packs: Vec<ThemePack>,
}

impl ThemeRegistry {
    pub fn load(explicit: Option<&Path>) -> Result<Self> {
        let mut packs = Vec::new();
        if let Some(path) = explicit {
            packs.push(ThemePack::load(Some(path))?);
        }
        for installed in crate::pack::installed(crate::pack::PackKind::Theme) {
            let manifest = installed.path.join(crate::pack::PACK_FILE_NAME);
            match ThemePack::load(Some(&manifest)) {
                Ok(pack) => packs.push(pack),
                Err(error) => {
                    eprintln!("提示：跳过已安装的主题包 {}：{error:#}", installed.meta.id);
                }
            }
        }
        packs.push(ThemePack::builtin()?);
        Ok(Self { packs })
    }

    /// 内置主题包（`coderender themes` 列表用）。
    pub fn builtin(&self) -> &ThemePack {
        self.packs.last().expect("注册表至少包含内置主题包")
    }

    /// 已安装的主题包（不含内置）。
    pub fn installed(&self) -> &[ThemePack] {
        &self.packs[..self.packs.len() - 1]
    }

    pub fn palette(&self, id: &str) -> Result<ResolvedPalette> {
        let mut candidates: Vec<String> = Vec::new();
        for pack in &self.packs {
            match pack.palette(id) {
                Ok(palette) => return Ok(palette),
                Err(_) => candidates.extend(
                    pack.palettes
                        .keys()
                        .cloned()
                        .chain(pack.palettes.values().filter_map(|p| p.name.clone())),
                ),
            }
        }
        candidates.sort_by_key(|name| name.to_lowercase());
        candidates.dedup();
        Err(anyhow!(
            "找不到配色预设 `{id}`{}。可用 `coderender themes` 查看所有预设。",
            crate::dym::did_you_mean(id, &candidates, 4)
        ))
    }

    pub fn decoration(&self, id: &str) -> Result<ResolvedDecoration> {
        let mut candidates: Vec<String> = Vec::new();
        for pack in &self.packs {
            match pack.decoration(id) {
                Ok(decoration) => return Ok(decoration),
                Err(_) => candidates.extend(
                    pack.decorations
                        .keys()
                        .cloned()
                        .chain(pack.decorations.values().filter_map(|d| d.name.clone())),
                ),
            }
        }
        candidates.sort_by_key(|name| name.to_lowercase());
        candidates.dedup();
        Err(anyhow!(
            "找不到窗口装饰预设 `{id}`{}。可用 `coderender themes` 查看所有预设。",
            crate::dym::did_you_mean(id, &candidates, 4)
        ))
    }
}

/// 归一化 id / 字体名：忽略大小写、空格、连字符和常见重音符号。
pub fn fold_key(input: &str) -> String {    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        let folded = match ch {
            'à'..='å' | 'À'..='Å' => 'a',
            'è'..='ë' | 'È'..='Ë' => 'e',
            'ì'..='ï' | 'Ì'..='Ï' => 'i',
            'ò'..='ö' | 'Ò'..='Ö' => 'o',
            'ù'..='ü' | 'Ù'..='Ü' => 'u',
            'ç' | 'Ç' => 'c',
            'ñ' | 'Ñ' => 'n',
            other => other,
        };
        if folded.is_alphanumeric() {
            out.extend(folded.to_lowercase());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_pack_is_valid() {
        let pack = ThemePack::builtin().unwrap();
        assert_eq!(pack.palettes.len(), 4);
        assert!(pack.palettes.contains_key("mocha"));
        assert!(pack.decorations.contains_key("macos"));
        assert!(pack.decorations.contains_key("windows"));
        assert!(pack.decorations.contains_key("none"));
    }

    #[test]
    fn resolves_default_palette_and_decoration() {
        let pack = ThemePack::builtin().unwrap();
        let palette = pack.palette(DEFAULT_PALETTE).unwrap();
        assert_eq!(palette.background, Rgba::opaque(0x1e, 0x1e, 0x2e));
        assert_eq!(palette.token(TokenKind::Keyword), Rgba::opaque(0xcb, 0xa6, 0xf7));
        let decor = pack.decoration(DEFAULT_DECORATION).unwrap();
        assert_eq!(decor.title_height, 36.0);
        assert!(matches!(decor.controls, WindowControls::TrafficLights { .. }));
    }

    #[test]
    fn accepts_accented_and_sloppy_ids() {
        let pack = ThemePack::builtin().unwrap();
        assert_eq!(pack.palette("Frappé").unwrap().id, "frappe");
        assert_eq!(pack.palette("MACCHIATO").unwrap().id, "macchiato");
        assert_eq!(
            pack.decoration("Windows 11").unwrap().id,
            "windows"
        );
        assert!(pack.palette("solarized").is_err());
    }
}
