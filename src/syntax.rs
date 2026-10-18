//! 语法高亮：syntect（TextMate 语法）→ 带颜色的字节区间。
//!
//! 高亮规则来自一个「语法注册表」：内置语法集 + 用户安装/显式指定的
//! 代码高亮扩展包（见 [`crate::syntaxpack`]）。查找顺序是
//! 显式包 → 已安装包 → 内置，这样扩展包既能补充新语言，也能覆盖内置语法。
//!
//! 性能说明：TextMate 语法是有状态的（第 N 行的高亮依赖前面所有行），
//! 因此这里逐行顺序解析、不做并行；但每行只产生少量 span，
//! 2000 行代码通常在 20ms 量级，真正的瓶颈在后续的字形栅格化。

use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{bail, Context, Result};
use syntect::easy::HighlightLines;
use syntect::highlighting::{
    FontStyle, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings,
};
use syntect::parsing::SyntaxSet;

use crate::color::Rgba;
use crate::dym;
use crate::syntaxpack::{self, SyntaxPack};
use crate::theme::{ResolvedPalette, TokenKind};

/// 内置代码高亮扩展包的清单（元信息）。
const BUILTIN_SYNTAX_PACK_TOML: &str = include_str!("../assets/syntax/builtin/pack.toml");
/// 内置代码高亮扩展包的语法数据（syntect 二进制 dump）。
const BUILTIN_SYNTAX_PACK_DUMP: &[u8] =
    include_bytes!("../assets/syntax/builtin/syntaxes/syntaxes.packdump");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenSpan {
    pub start: usize,
    pub end: usize,
    pub fg: Rgba,
    pub italic: bool,
    pub bold: bool,
}

/// (选择器, 颜色类别, 是否斜体)。syntect 会按选择器特异性打分，顺序不敏感。
const RULES: &[(&str, TokenKind, bool)] = &[
    ("comment, punctuation.definition.comment", TokenKind::Comment, true),
    ("constant.character.escape, string.regexp", TokenKind::Escape, false),
    ("constant.numeric", TokenKind::Number, false),
    ("constant.language, constant.other", TokenKind::Constant, false),
    ("constant", TokenKind::Constant, false),
    ("string", TokenKind::String, false),
    ("keyword.operator", TokenKind::Operator, false),
    ("keyword", TokenKind::Keyword, false),
    (
        "entity.name.function, support.function, variable.function, meta.function-call",
        TokenKind::Function,
        false,
    ),
    (
        "entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, entity.name.trait, entity.name.interface, entity.name.impl, entity.name.namespace",
        TokenKind::Type,
        false,
    ),
    ("meta.generic, support.type, support.class, storage.type", TokenKind::Type, false),
    // 具体的内置标量类型：Type（黄色）
    (
        "storage.type.numeric, storage.type.primitive, storage.type.bool, storage.type.string, storage.type.char, storage.type.float, storage.type.integer",
        TokenKind::Type,
        false,
    ),
    // 语言关键字伪装成 storage.*（Rust 的 fn / struct / impl / pub / let 等）：
    // 这些比上面的 storage.type 更具体，会优先命中。
    (
        "storage.type.function, storage.type.struct, storage.type.enum, storage.type.trait, storage.type.impl, storage.type.mod, storage.type.union, storage.type.rust, storage.modifier",
        TokenKind::Keyword,
        false,
    ),
    (
        "support.constant, support.function.builtin, support.type.builtin, variable.language",
        TokenKind::Builtin,
        false,
    ),
    ("entity.name.tag", TokenKind::Tag, false),
    (
        "entity.other.attribute-name, meta.annotation, meta.attribute, variable.annotation, storage.modifier.attribute",
        TokenKind::Attribute,
        false,
    ),
    ("variable.parameter", TokenKind::Parameter, false),
    (
        "variable.other.member, variable.other.property, variable.other.object.property, meta.object-literal.key, support.variable.property",
        TokenKind::Property,
        false,
    ),
    ("storage", TokenKind::Keyword, false),
    ("invalid", TokenKind::Invalid, false),
    ("punctuation", TokenKind::Punctuation, false),
    ("variable", TokenKind::Variable, false),
];

// ---------------------------------------------------------------- 注册表

/// 一组语法 + 它的来源描述。
struct LoadedSet {
    set: SyntaxSet,
    origin: String,
    extensions: Vec<String>,
    names: Vec<String>,
}

/// 语法查找结果：定位到某个语法集里的某个语法。
#[derive(Debug, Clone)]
pub struct Picked {
    pub set_index: usize,
    pub syntax_index: usize,
    pub language: String,
    /// 来源描述：`内置` / 包名。
    pub origin: String,
}

/// 高亮规则注册表：内置 + 扩展包。
pub struct SyntaxRegistry {
    sets: Vec<LoadedSet>,
}

impl SyntaxRegistry {
    /// 加载注册表。
    ///
    /// * `explicit`：`--syntax-pack` 指定的单个包（目录或 `.crpack`）
    /// * `include_installed`：是否加载用户已安装的扩展包
    pub fn load(explicit: Option<&Path>, include_installed: bool) -> Result<Self> {
        let mut sets: Vec<LoadedSet> = Vec::new();

        // 1. 显式指定的包优先级最高（可以覆盖内置语法）
        if let Some(path) = explicit {
            if !path.exists() {
                bail!("找不到代码高亮扩展包 {}", path.display());
            }
            let pack = load_pack_from(path)?;
            let built = pack.build_syntax_sets()?;
            let origin = format!("{}（{}）", pack.meta.name, path.display());
            for set in built {
                sets.push(LoadedSet {
                    set,
                    origin: origin.clone(),
                    extensions: pack.extensions.clone(),
                    names: pack.names.clone(),
                });
            }
        }

        // 2. 用户安装的扩展包
        if include_installed {
            for pack in crate::pack::installed(crate::pack::PackKind::Syntax) {
                let loaded = SyntaxPack::load(&pack.path)
                    .with_context(|| format!("加载已安装的扩展包 {} 失败", pack.path.display()))?;
                match loaded.build_syntax_sets() {
                    Ok(built) => {
                        let origin = format!("{}（{}）", pack.meta.name, pack.meta.id);
                        for set in built {
                            sets.push(LoadedSet {
                                set,
                                origin: origin.clone(),
                                extensions: loaded.extensions.clone(),
                                names: loaded.names.clone(),
                            });
                        }
                    }
                    Err(error) => {
                        eprintln!("提示：跳过扩展包 {}：{error:#}", pack.meta.id);
                    }
                }
            }
        }

        // 3. 内置语法集（以「内置代码高亮扩展包」的形式提供）
        sets.push(LoadedSet {
            set: builtin_syntax_set(),
            origin: "内置".to_string(),
            extensions: Vec::new(),
            names: Vec::new(),
        });

        Ok(Self { sets })
    }

    pub fn set(&self, index: usize) -> &SyntaxSet {
        &self.sets[index].set
    }

    /// 所有来源描述（供 --verbose 输出）。
    pub fn origins(&self) -> Vec<&str> {
        self.sets.iter().map(|set| set.origin.as_str()).collect()
    }

    /// 所有已知的语言名与扩展名（用于模糊提示）。
    pub fn language_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .sets
            .iter()
            .flat_map(|loaded| {
                syntaxpack::describe_syntaxes(&loaded.set)
                    .into_iter()
                    .map(|(name, _)| name)
                    .chain(loaded.names.iter().cloned())
                    .chain(loaded.extensions.iter().cloned())
            })
            .collect();
        names.sort_by_key(|name| name.to_lowercase());
        names.dedup();
        names
    }

    /// 按 `--lang` / 文件扩展名选择语法；都不命中时回退到纯文本。
    pub fn pick(&self, lang: Option<&str>, path: Option<&Path>) -> Result<Picked> {
        if let Some(lang) = lang {
            for (set_index, loaded) in self.sets.iter().enumerate() {
                if let Some(syntax) = find_by_lang(&loaded.set, lang) {
                    return Ok(Picked {
                        set_index,
                        syntax_index: syntax,
                        language: self.sets[set_index].set.syntaxes()[syntax].name.clone(),
                        origin: loaded.origin.clone(),
                    });
                }
                // 扩展包在 pack.toml 里补充的 extensions / names
                if let Some(syntax) = find_by_declared(&loaded.set, lang, &loaded.extensions, &loaded.names)
                {
                    return Ok(Picked {
                        set_index,
                        syntax_index: syntax,
                        language: self.sets[set_index].set.syntaxes()[syntax].name.clone(),
                        origin: loaded.origin.clone(),
                    });
                }
            }
            // 语言拼错：给出「您是否指的是……」并退出
            let candidates = self.language_names();
            bail!(
                "找不到语言/语法 `{lang}`{}。可用 `coderender pack list` 查看已安装的代码高亮扩展包。",
                dym::did_you_mean(lang, &candidates, 3)
            );
        }

        if let Some(extension) = path
            .and_then(|path| path.extension())
            .and_then(|extension| extension.to_str())
        {
            for (set_index, loaded) in self.sets.iter().enumerate() {
                if let Some(syntax) = loaded.set.find_syntax_by_extension(extension) {
                    let index = loaded
                        .set
                        .syntaxes()
                        .iter()
                        .position(|candidate| std::ptr::eq(candidate, syntax))
                        .unwrap_or(0);
                    return Ok(Picked {
                        set_index,
                        syntax_index: index,
                        language: syntax.name.clone(),
                        origin: loaded.origin.clone(),
                    });
                }
            }
        }

        Ok(Picked {
            set_index: self.sets.len() - 1,
            syntax_index: plain_text_index(self.set(self.sets.len() - 1)),
            language: "Plain Text".to_string(),
            origin: self.sets[self.sets.len() - 1].origin.clone(),
        })
    }

    /// 内置代码高亮扩展包的清单内容（供 `pack list` / `pack builtin-export`）。
    pub fn builtin_pack_toml() -> &'static str {
        BUILTIN_SYNTAX_PACK_TOML
    }

    /// 把内置语法规则导出成一个「代码高亮扩展包」目录（pack.toml + dump）。
    pub fn export_builtin(dir: &Path) -> Result<PathBuf> {
        std::fs::create_dir_all(dir.join("syntaxes"))
            .with_context(|| format!("无法创建 {}", dir.display()))?;
        let dump = dir.join("syntaxes").join("syntaxes.packdump");
        syntect::dumps::dump_to_file(&builtin_syntax_set(), &dump)
            .with_context(|| format!("无法写入 {}", dump.display()))?;
        let manifest = dir.join(crate::pack::PACK_FILE_NAME);
        let extra = format!(
            "# 本包由 `coderender pack builtin-export` 生成：{} 个语法。\n{}",
            builtin_syntax_set().syntaxes().len(),
            BUILTIN_SYNTAX_PACK_TOML
        );
        std::fs::write(&manifest, extra)
            .with_context(|| format!("无法写入 {}", manifest.display()))?;
        Ok(dir.to_path_buf())
    }
}

fn load_pack_from(path: &Path) -> Result<SyntaxPack> {
    if path.is_dir() {
        return SyntaxPack::load(path);
    }
    if crate::pack::is_distribution(path) {
        let unpacked = crate::pack::data_dir().join(".unpacking");
        let _ = std::fs::remove_dir_all(&unpacked);
        std::fs::create_dir_all(&unpacked)?;
        crate::pack::extract_distribution(path, &unpacked)?;
        let root = crate::pack::single_subdirectory(&unpacked).unwrap_or(unpacked.clone());
        let result = SyntaxPack::load(&root);
        let _ = std::fs::remove_dir_all(&unpacked);
        result
    } else {
        bail!(
            "无法识别的代码高亮扩展包 `{}`（支持目录或 .{}）",
            path.display(),
            crate::pack::PACK_EXTENSION
        )
    }
}

/// 内置语法集：优先用扩展包里的 dump，失败时回落到 syntect 默认语法集。
fn builtin_syntax_set() -> SyntaxSet {
    let cursor = std::io::Cursor::new(BUILTIN_SYNTAX_PACK_DUMP);
    match syntect::dumps::from_reader::<SyntaxSet, _>(cursor) {
        Ok(set) => set,
        Err(error) => {
            eprintln!("提示：内置代码高亮扩展包数据损坏（{error}），回落到 syntect 默认语法集");
            SyntaxSet::load_defaults_newlines()
        }
    }
}

fn find_by_lang(set: &SyntaxSet, lang: &str) -> Option<usize> {
    set.syntaxes()
        .iter()
        .position(|syntax| {
            syntax.name.eq_ignore_ascii_case(lang)
                || syntax.file_extensions.iter().any(|ext| ext.eq_ignore_ascii_case(lang))
        })
        .or_else(|| set.syntaxes().iter().position(|s| s.name.to_lowercase() == lang.to_lowercase()))
}

fn find_by_declared(
    set: &SyntaxSet,
    lang: &str,
    extensions: &[String],
    names: &[String],
) -> Option<usize> {
    let folded = lang.to_lowercase();
    let hits_extension = extensions.iter().any(|ext| ext.to_lowercase() == folded);
    let hits_name = names
        .iter()
        .any(|name| name.to_lowercase() == folded || name.to_lowercase().contains(&folded));
    if !hits_extension && !hits_name {
        return None;
    }
    // 包里补充的 extensions/names 指向哪个语法：取第一个非纯文本语法
    set.syntaxes()
        .iter()
        .position(|syntax| syntax.name != "Plain Text")
}

fn plain_text_index(set: &SyntaxSet) -> usize {
    set.syntaxes()
        .iter()
        .position(|syntax| syntax.name == "Plain Text")
        .unwrap_or(0)
}

// ---------------------------------------------------------------- 高亮器

pub struct Highlighter<'a> {
    syntax_set: &'a SyntaxSet,
    syntax_index: usize,
    theme: Theme,
    pub language: String,
    /// 语法来源（内置 / 扩展包名）。
    pub origin: String,
}

impl<'a> Highlighter<'a> {
    pub fn new(
        registry: &'a SyntaxRegistry,
        palette: &ResolvedPalette,
        lang: Option<&str>,
        path: Option<&Path>,
    ) -> Result<Self> {
        let picked = registry.pick(lang, path)?;
        Ok(Self {
            syntax_set: registry.set(picked.set_index),
            syntax_index: picked.syntax_index,
            language: picked.language,
            origin: picked.origin,
            theme: syntect_theme(palette),
        })
    }

    pub fn is_plain_text(&self) -> bool {
        self.language == "Plain Text"
    }

    pub fn highlight(&self, text: &str, palette: &ResolvedPalette) -> Result<Vec<TokenSpan>> {
        if text.is_empty() {
            return Ok(Vec::new());
        }
        // 纯文本没有语法，直接一个 span，省掉逐行解析。
        if self.is_plain_text() {
            return Ok(vec![TokenSpan {
                start: 0,
                end: text.len(),
                fg: palette.foreground,
                italic: false,
                bold: false,
            }]);
        }

        let syntax = &self.syntax_set.syntaxes()[self.syntax_index];
        let mut highlighter = HighlightLines::new(syntax, &self.theme);
        let mut spans: Vec<TokenSpan> = Vec::with_capacity(text.len() / 12 + 16);
        let mut line_start = 0usize;

        for line in text.split_inclusive('\n') {
            let regions = highlighter
                .highlight_line(line, self.syntax_set)
                .with_context(|| format!("语法高亮失败（{}）", self.language))?;
            let mut consumed = 0usize;
            for (style, piece) in regions {
                let len = piece.len();
                if len == 0 {
                    continue;
                }
                let start = line_start + consumed;
                consumed += len;
                push_merged(
                    &mut spans,
                    TokenSpan {
                        start,
                        end: start + len,
                        fg: style.foreground.into(),
                        italic: style.font_style.contains(FontStyle::ITALIC),
                        bold: style.font_style.contains(FontStyle::BOLD),
                    },
                );
            }
            line_start += line.len();
        }

        Ok(spans)
    }
}

fn push_merged(spans: &mut Vec<TokenSpan>, span: TokenSpan) {
    if let Some(last) = spans.last_mut() {
        if last.end == span.start
            && last.fg == span.fg
            && last.italic == span.italic
            && last.bold == span.bold
        {
            last.end = span.end;
            return;
        }
    }
    spans.push(span);
}

/// 把配色预设转成 syntect 主题（供调试/自定义高亮复用）。
pub fn syntect_theme(palette: &ResolvedPalette) -> Theme {
    let scopes: Vec<ThemeItem> = RULES
        .iter()
        .map(|(selector, kind, italic)| ThemeItem {
            scope: ScopeSelectors::from_str(selector).expect("内置 syntect 选择器应始终合法"),
            style: StyleModifier {
                foreground: Some(syntect::highlighting::Color {
                    r: palette.token(*kind).r,
                    g: palette.token(*kind).g,
                    b: palette.token(*kind).b,
                    a: palette.token(*kind).a,
                }),
                background: None,
                font_style: italic.then_some(FontStyle::ITALIC),
            },
        })
        .collect();

    Theme {
        name: Some(palette.name.clone()),
        author: None,
        settings: ThemeSettings {
            foreground: Some(syntect::highlighting::Color {
                r: palette.foreground.r,
                g: palette.foreground.g,
                b: palette.foreground.b,
                a: palette.foreground.a,
            }),
            background: Some(syntect::highlighting::Color {
                r: palette.background.r,
                g: palette.background.g,
                b: palette.background.b,
                a: palette.background.a,
            }),
            ..ThemeSettings::default()
        },
        scopes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemePack;

    fn registry() -> SyntaxRegistry {
        SyntaxRegistry::load(None, false).unwrap()
    }

    #[test]
    fn builtin_pack_is_available() {
        let registry = registry();
        assert!(!registry.origins().is_empty());
        assert!(registry.language_names().len() > 20);
        // 内置语法来自内置扩展包
        let picked = registry.pick(Some("rust"), None).unwrap();
        assert_eq!(picked.language, "Rust");
        assert_eq!(picked.origin, "内置");
    }

    #[test]
    fn highlights_rust_and_keeps_offsets_contiguous() {
        let pack = ThemePack::builtin().unwrap();
        let palette = pack.palette("mocha").unwrap();
        let registry = registry();
        let highlighter = Highlighter::new(&registry, &palette, Some("rs"), None).unwrap();
        let code = "// hi\nfn main() {\n    let x = 42;\n}\n";
        let spans = highlighter.highlight(code, &palette).unwrap();
        assert_eq!(spans.first().unwrap().start, 0);
        assert_eq!(spans.last().unwrap().end, code.len());
        for pair in spans.windows(2) {
            assert_eq!(pair[0].end, pair[1].start);
        }
        assert!(spans.iter().any(|span| span.italic));
    }

    #[test]
    fn plain_text_is_single_span() {
        let pack = ThemePack::builtin().unwrap();
        let palette = pack.palette("latte").unwrap();
        let registry = registry();
        let highlighter = Highlighter::new(&registry, &palette, Some("txt"), None).unwrap();
        let text = "hello\nworld\n";
        let spans = highlighter.highlight(text, &palette).unwrap();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].end, text.len());
    }

    #[test]
    fn picks_by_file_extension() {
        let registry = registry();
        let picked = registry.pick(None, Some(Path::new("main.rs"))).unwrap();
        assert_eq!(picked.language, "Rust");
    }

    #[test]
    fn typo_in_lang_fails_with_suggestion() {
        let registry = registry();
        let error = registry.pick(Some("rustt"), None).unwrap_err().to_string();
        assert!(error.contains("您是否指的是"), "{error}");
        assert!(error.contains("Rust"), "{error}");
    }

    #[test]
    fn exports_builtin_pack_to_disk() {
        let dir = std::env::temp_dir().join("coderender-builtin-export");
        let _ = std::fs::remove_dir_all(&dir);
        SyntaxRegistry::export_builtin(&dir).unwrap();
        assert!(dir.join(crate::pack::PACK_FILE_NAME).is_file());
        assert!(dir.join("syntaxes").join("syntaxes.packdump").is_file());

        // 导出的包可以再次被加载
        let pack = SyntaxPack::load(&dir).unwrap();
        assert_eq!(pack.meta.kind, crate::pack::PackKind::Syntax);
        let sets = pack.build_syntax_sets().unwrap();
        assert!(sets[0].find_syntax_by_name("Rust").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explicit_pack_adds_new_language() {
        let dir = std::env::temp_dir().join("coderender-explicit-pack");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("syntaxes")).unwrap();
        std::fs::write(
            dir.join(crate::pack::PACK_FILE_NAME),
            "[pack]\nid = \"demo-lang\"\nkind = \"syntax\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("syntaxes").join("Demo.sublime-syntax"),
            "name: DemoLang\nfile_extensions: [demo]\nscope: source.demo\ncontexts:\n  main:\n    - match: '#.*'\n      scope: comment.line.demo\n",
        )
        .unwrap();

        let registry = SyntaxRegistry::load(Some(&dir), false).unwrap();
        let picked = registry.pick(Some("demolang"), None).unwrap();
        assert_eq!(picked.language, "DemoLang");
        assert!(picked.origin.contains("demo-lang") || picked.origin.contains("Demo"));
        let by_extension = registry.pick(None, Some(Path::new("a.demo"))).unwrap();
        assert_eq!(by_extension.language, "DemoLang");
        // 内置语言仍然可用
        assert_eq!(registry.pick(Some("rust"), None).unwrap().language, "Rust");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
