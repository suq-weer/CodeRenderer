//! 代码高亮扩展包：让渲染器认识 syntect 默认语法集之外的语言。
//!
//! 一个扩展包是一个「开发文件夹」：
//!
//! ```text
//! my-syntax-pack/
//!   pack.toml                       # [pack] + 可选的 [[syntax]]
//!   syntaxes/
//!     Zig.sublime-syntax
//!     Gleam.sublime-syntax
//! ```
//!
//! `pack.toml` 里省略 `[[syntax]]` 时，程序自动扫描 `syntaxes/` 下的
//! `*.sublime-syntax`。安装后的包放在
//! `<data_dir>/coderender/packs/syntax/<id>/`，也可以用
//! `--syntax-pack <目录>` 一次性加载某个包。

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use syntect::parsing::{SyntaxDefinition, SyntaxSet, SyntaxSetBuilder};

use crate::pack::{self, PackMeta};

#[derive(Debug, Clone, Deserialize)]
struct RawSyntaxPack {
    pack: RawPackSection,
    /// 显式声明要加载的语法文件（相对包根目录）。
    #[serde(default, alias = "syntaxes")]
    syntax: Vec<RawSyntaxEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawPackSection {
    id: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    license: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawSyntaxEntry {
    path: String,
    /// 补充的扩展名（语法文件里已经声明过的可以省略）。
    #[serde(default)]
    extensions: Vec<String>,
    /// 补充的语言名。
    #[serde(default)]
    names: Vec<String>,
}

/// 一个已加载的代码高亮扩展包。
#[derive(Debug, Clone)]
pub struct SyntaxPack {
    pub meta: PackMeta,
    /// 包根目录。
    pub root: PathBuf,
    /// 语法文件（相对包根目录，已解析为绝对路径）。
    pub files: Vec<PathBuf>,
    /// 额外声明的扩展名 / 语言名（用于 `--lang` 匹配）。
    pub extensions: Vec<String>,
    pub names: Vec<String>,
}

impl SyntaxPack {
    /// 从开发文件夹（或已安装目录）加载扩展包。
    pub fn load(dir: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(dir.join(pack::PACK_FILE_NAME))
            .with_context(|| format!("无法读取 {}", dir.join(pack::PACK_FILE_NAME).display()))?;
        let raw: RawSyntaxPack = toml::from_str(&text)
            .with_context(|| format!("{} 解析失败", dir.join(pack::PACK_FILE_NAME).display()))?;

        if raw.pack.id.trim().is_empty() {
            bail!("pack.id 不能为空");
        }
        match raw.pack.kind.as_deref() {
            Some("syntax") | Some("syntaxes") | Some("highlight") | None => {}
            Some(other) => bail!("pack.kind = `{other}`：代码高亮扩展包必须是 syntax"),
        }

        let mut files = Vec::new();
        let mut extensions = Vec::new();
        let mut names = Vec::new();

        for entry in &raw.syntax {
            let path = dir.join(&entry.path);
            if !path.is_file() {
                bail!(
                    "[[syntax]] 声明的文件不存在：{}（相对于 {}）",
                    path.display(),
                    dir.display()
                );
            }
            files.push(path);
            extensions.extend(entry.extensions.iter().cloned());
            names.extend(entry.names.iter().cloned());
        }

        // 没有显式声明时扫描 syntaxes/ 目录
        if files.is_empty() {
            let syntaxes = dir.join("syntaxes");
            if syntaxes.is_dir() {
                collect_syntax_files(&syntaxes, &mut files)?;
            }
            if files.is_empty() {
                bail!(
                    "{} 里没有任何语法文件：请在 pack.toml 里写 [[syntax]]，或在 syntaxes/ 放 *.sublime-syntax",
                    dir.display()
                );
            }
        }

        Ok(Self {
            meta: PackMeta {
                id: raw.pack.id.clone(),
                kind: pack::PackKind::Syntax,
                name: raw
                    .pack
                    .name
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or(raw.pack.id),
                version: raw.pack.version.filter(|v| !v.trim().is_empty()),
                description: raw.pack.description.filter(|d| !d.trim().is_empty()),
                license: raw.pack.license.filter(|l| !l.trim().is_empty()),
            },
            root: dir.to_path_buf(),
            files,
            extensions,
            names,
        })
    }

    /// 构建语法集：`.sublime-syntax` 合并成一个 `SyntaxSet`；
    /// `.packdump`（内置包用的二进制格式）各自成为一个 `SyntaxSet`。
    pub fn build_syntax_sets(&self) -> Result<Vec<SyntaxSet>> {
        let mut sets: Vec<SyntaxSet> = Vec::new();
        let mut builder = SyntaxSetBuilder::new();
        let mut yaml_count = 0usize;

        for file in &self.files {
            let extension = file
                .extension()
                .map(|ext| ext.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            match extension.as_str() {
                "packdump" | "dump" => {
                    let bytes = std::fs::read(file)
                        .with_context(|| format!("读取 {}", file.display()))?;
                    let set: SyntaxSet = syntect::dumps::from_binary(&bytes);
                    sets.push(set);
                }
                _ => {
                    let text = std::fs::read_to_string(file)
                        .with_context(|| format!("读取 {} 失败", file.display()))?;
                    let name = file
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .unwrap_or("syntax");
                    let definition = SyntaxDefinition::load_from_str(&text, true, Some(name))
                        .map_err(|error| anyhow::anyhow!("{}：{error}", file.display()))?;
                    builder.add(definition);
                    yaml_count += 1;
                }
            }
        }

        if yaml_count > 0 {
            // 保证 find_syntax_plain_text() 有东西可返回
            let has_plain_text = builder
                .syntaxes()
                .iter()
                .any(|definition| definition.name == "Plain Text");
            if !has_plain_text {
                builder.add_plain_text_syntax();
            }
            sets.push(builder.build());
        }

        if sets.is_empty() {
            bail!("{} 没有生成任何语法", self.root.display());
        }
        Ok(sets)
    }
}

fn collect_syntax_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?
        .flatten()
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_syntax_files(&path, out)?;
        } else if path
            .extension()
            .map(|ext| ext.eq_ignore_ascii_case("sublime-syntax"))
            .unwrap_or(false)
        {
            out.push(path);
        }
    }
    Ok(())
}

/// 语法集里所有语言的（名称, 扩展名）清单，用于 `--lang` 提示与 `pack info`。
pub fn describe_syntaxes(set: &SyntaxSet) -> Vec<(String, Vec<String>)> {
    set.syntaxes()
        .iter()
        .map(|syntax| (syntax.name.clone(), syntax.file_extensions.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::create_template;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("coderender-syntaxpack-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn loads_template_and_highlights_its_syntax() {
        let dir = temp("template");
        let created = create_template("MyLang Extra", crate::pack::PackKind::Syntax, Some(&dir))
            .unwrap();
        let pack = SyntaxPack::load(&created).unwrap();
        assert_eq!(pack.meta.kind, crate::pack::PackKind::Syntax);
        assert_eq!(pack.files.len(), 1);

        let sets = pack.build_syntax_sets().unwrap();
        assert_eq!(sets.len(), 1);
        let set = &sets[0];
        assert!(set.find_syntax_by_name("MyLang").is_some());
        assert!(set.find_syntax_by_extension("mylang").is_some());

        // 用它高亮一段自定义语言代码
        let theme = default_theme();
        let syntax = set.find_syntax_by_extension("mylang").unwrap();
        let mut highlighter = syntect::easy::HighlightLines::new(syntax, &theme);
        let regions = highlighter
            .highlight_line("let x = 42 # hi\n", set)
            .unwrap();
        let text: String = regions.iter().map(|(_, t)| *t).collect();
        assert_eq!(text, "let x = 42 # hi\n");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scans_syntaxes_folder_without_declarations() {
        let dir = temp("scan");
        std::fs::create_dir_all(dir.join("syntaxes")).unwrap();
        std::fs::write(
            dir.join("pack.toml"),
            "[pack]\nid = \"scan-me\"\nkind = \"syntax\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("syntaxes").join("A.sublime-syntax"),
            "name: LangA\nfile_extensions: [la]\nscope: source.la\ncontexts:\n  main: []\n",
        )
        .unwrap();
        let pack = SyntaxPack::load(&dir).unwrap();
        assert_eq!(pack.files.len(), 1);
        let sets = pack.build_syntax_sets().unwrap();
        assert!(sets[0].find_syntax_by_extension("la").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_pack_without_syntax_files() {
        let dir = temp("empty");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("pack.toml"),
            "[pack]\nid = \"empty\"\nkind = \"syntax\"\n",
        )
        .unwrap();
        assert!(SyntaxPack::load(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_wrong_kind() {
        let dir = temp("wrongkind");
        std::fs::create_dir_all(dir.join("syntaxes")).unwrap();
        std::fs::write(
            dir.join("pack.toml"),
            "[pack]\nid = \"wk\"\nkind = \"theme\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("syntaxes").join("B.sublime-syntax"),
            "name: LangB\nscope: source.b\ncontexts:\n  main: []\n",
        )
        .unwrap();
        assert!(SyntaxPack::load(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn default_theme() -> syntect::highlighting::Theme {
        let pack = crate::theme::ThemePack::builtin().unwrap();
        let palette = pack.palette("mocha").unwrap();
        crate::syntax::syntect_theme(&palette)
    }
}
