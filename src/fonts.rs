//! 字体：系统字体枚举 / 解析，以及内置的 JetBrainsMono Nerd Font。
//!
//! 内置字体来自 [JetBrains Mono](https://github.com/JetBrains/JetBrainsMono) 的
//! Nerd Font patched 版本，许可证为 SIL Open Font License 1.1（见
//! `assets/fonts/OFL.txt`，`coderender fonts --license` 可打印）。

use std::collections::BTreeMap;

use anyhow::{bail, Context, Result};
use cosmic_text::{fontdb, FontSystem};

use crate::dym;
use crate::theme::fold_key;

pub const FONT_LICENSE: &str = include_str!("../assets/fonts/OFL.txt");

const REGULAR: &[u8] = include_bytes!("../assets/fonts/JetBrainsMonoNerdFontMono-Regular.ttf");
const BOLD: &[u8] = include_bytes!("../assets/fonts/JetBrainsMonoNerdFontMono-Bold.ttf");
const ITALIC: &[u8] = include_bytes!("../assets/fonts/JetBrainsMonoNerdFontMono-Italic.ttf");

/// 默认等宽字体候选（按优先级）。JetBrainsMono Nerd Font 已内置于二进制中，
/// 因此即使系统没安装也能命中。
pub const DEFAULT_MONO_CANDIDATES: &[&str] = &[
    "JetBrainsMono Nerd Font Mono",
    "JetBrainsMono Nerd Font",
    "JetBrains Mono",
    "Cascadia Mono",
    "Cascadia Code",
    "Consolas",
    "Menlo",
    "SF Mono",
    "DejaVu Sans Mono",
    "Ubuntu Mono",
    "Liberation Mono",
];

/// 非等宽回退字体候选：代码中的中文等字符使用它排印。
pub const DEFAULT_FALLBACK_CANDIDATES: &[&str] = &[
    "Microsoft YaHei UI",
    "Microsoft YaHei",
    "PingFang SC",
    "Hiragino Sans GB",
    "Noto Sans CJK SC",
    "Noto Sans SC",
    "Source Han Sans SC",
    "Source Han Sans CN",
    "Segoe UI",
    "SF Pro Text",
    "Helvetica Neue",
    "Noto Sans",
    "DejaVu Sans",
    "Arial",
];

/// 创建 FontSystem：枚举系统字体，再注册内置字体（重复注册无害，
/// 系统已安装时会优先命中系统字形）。
pub fn font_system() -> FontSystem {
    let mut system = FontSystem::new();
    let db = system.db_mut();
    for data in [REGULAR, BOLD, ITALIC] {
        db.load_font_data(data.to_vec());
    }
    system
}

/// 字体名 → 真实族名索引（按归一化名匹配，容忍大小写/空格/重音差异）。
struct FamilyIndex {
    all: BTreeMap<String, String>,
    mono: BTreeMap<String, String>,
}

impl FamilyIndex {
    fn build(db: &fontdb::Database) -> Self {
        let mut all = BTreeMap::new();
        let mut mono = BTreeMap::new();
        for face in db.faces() {
            for (family, _) in &face.families {
                let key = fold_key(family);
                if key.is_empty() {
                    continue;
                }
                all.entry(key.clone()).or_insert_with(|| family.clone());
                if face.monospaced {
                    mono.entry(key).or_insert_with(|| family.clone());
                }
            }
        }
        Self { all, mono }
    }

    fn names(&self, mono_only: bool) -> Vec<String> {
        let source = if mono_only { &self.mono } else { &self.all };
        let mut names: Vec<String> = source.values().cloned().collect();
        names.sort_by_key(|name| name.to_lowercase());
        names.dedup();
        names
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontSet {
    /// 代码字体（等宽）
    pub mono: String,
    /// 非等宽回退字体（CJK / 标题栏文字）
    pub fallback: String,
}

impl FontSet {
    pub fn resolve(system: &FontSystem, mono: Option<&str>, fallback: Option<&str>) -> Result<Self> {
        let db = system.db();
        let index = FamilyIndex::build(db);

        let mono = match mono {
            Some(name) => index
                .all
                .get(&fold_key(name))
                .cloned()
                .ok_or_else(|| unknown_font(name, &index, true))?,
            None => DEFAULT_MONO_CANDIDATES
                .iter()
                .find_map(|name| index.mono.get(&fold_key(name)).cloned())
                .or_else(|| index.mono.values().next().cloned())
                .ok_or_else(|| anyhow::anyhow!("系统未找到任何等宽字体，请用 --font-mono 指定"))?,
        };

        let fallback = match fallback {
            Some(name) => index
                .all
                .get(&fold_key(name))
                .cloned()
                .ok_or_else(|| unknown_font(name, &index, false))?,
            None => DEFAULT_FALLBACK_CANDIDATES
                .iter()
                .find_map(|name| index.all.get(&fold_key(name)).cloned())
                .unwrap_or_else(|| mono.clone()),
        };

        Ok(Self { mono, fallback })
    }

    /// 该族名是否为等宽字体（用于给出友好提示）。
    pub fn is_monospace(system: &FontSystem, family: &str) -> bool {
        let index = FamilyIndex::build(system.db());
        index.mono.contains_key(&fold_key(family))
    }
}

fn unknown_font(requested: &str, index: &FamilyIndex, mono_only: bool) -> anyhow::Error {
    let candidates = index.names(mono_only);
    let mut msg = format!("未找到字体 `{requested}`");
    if mono_only {
        msg.push_str("（等宽字体）");
    }
    msg.push_str(&dym::did_you_mean(requested, &candidates, 5));
    msg.push_str("。可用 `coderender fonts` 查看已安装字体。");
    anyhow::anyhow!(msg)
}

/// 列出字体族。`mono_only` 为真时只返回等宽字体。
pub fn list_families(system: &FontSystem, mono_only: bool) -> Vec<String> {
    FamilyIndex::build(system.db()).names(mono_only)
}

/// 校验用户显式指定的字体是否可用，并返回解析后的 FontSet。
pub fn resolve_checked(
    system: &FontSystem,
    mono: Option<&str>,
    fallback: Option<&str>,
) -> Result<FontSet> {
    let set = FontSet::resolve(system, mono, fallback)
        .context("字体解析失败（--font-mono / --font-fallback）")?;
    if let Some(requested) = mono {
        if !FontSet::is_monospace(system, &set.mono) {
            let index = FamilyIndex::build(system.db());
            let candidates = index.names(true);
            bail!(
                "`{requested}` 不是等宽字体，代码渲染需要等宽字体（已解析为 `{}`）{}。可用 `coderender fonts` 查看等宽字体。",
                set.mono,
                dym::did_you_mean(requested, &candidates, 5)
            );
        }
    }
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_fonts_resolve() {
        let system = font_system();
        let set = FontSet::resolve(&system, None, None).unwrap();
        assert!(set.mono.to_lowercase().contains("jetbrains"));
        assert!(!set.fallback.is_empty());
        assert!(FontSet::is_monospace(&system, &set.mono));
    }

    #[test]
    fn unknown_font_suggests_candidates() {
        let system = font_system();
        let error = FontSet::resolve(&system, Some("JetBrains Monoo"), None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("未找到字体"), "{error}");
        assert!(error.contains("您是否指的是"), "{error}");
        assert!(error.contains("JetBrainsMono Nerd Font Mono"), "{error}");
    }

    #[test]
    fn proportional_font_is_rejected_with_suggestion() {
        let system = font_system();
        let error = resolve_checked(&system, Some("Microsoft YaHei UI"), None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("不是等宽字体"), "{error}");
    }

    #[test]
    fn lists_monospace_families() {
        let system = font_system();
        let families = list_families(&system, true);
        assert!(families.iter().any(|f| f.to_lowercase().contains("jetbrains")));
    }
}
