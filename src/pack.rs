//! 扩展包管理：安装目录、开发文件夹、分发格式（`.crpack`）、安装/卸载/列表。
//!
//! 目录约定（可用环境变量 `CODERENDER_HOME` 覆盖，便于测试）：
//!
//! ```text
//! 用户级：<data_dir>/coderender/packs/{themes,syntax}/<id>/pack.toml
//! 机器级：%ProgramData%\coderender/packs/{themes,syntax}/<id>/pack.toml   （仅 Windows）
//! ```
//!
//! 查找时机器级优先（MSI 安装的共享插件），用户级其次（`coderender pack install` 装的）。
//! 安装到机器级需要管理员权限，`--scope machine` 可显式指定。
//!
//! 分发格式 `.crpack` 就是 zip：里面是「开发文件夹」的原始内容，
//! 因此 `pack build <目录>` 与 `pack install <目录|.crpack>` 完全对称。

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::dym;

/// 分发格式的扩展名。
pub const PACK_EXTENSION: &str = "crpack";
/// 开发文件夹/分发包内的清单文件名。
pub const PACK_FILE_NAME: &str = "pack.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackKind {
    Theme,
    Syntax,
}

impl PackKind {
    pub const ALL: [PackKind; 2] = [PackKind::Theme, PackKind::Syntax];

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_lowercase().as_str() {
            "theme" | "themes" | "主题" | "主题包" => Some(Self::Theme),
            "syntax" | "syntaxes" | "highlight" | "高亮" | "代码高亮" | "代码高亮扩展包" => {
                Some(Self::Syntax)
            }
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Theme => "theme",
            Self::Syntax => "syntax",
        }
    }

    /// 安装目录名。
    pub fn dir_name(self) -> &'static str {
        match self {
            Self::Theme => "themes",
            Self::Syntax => "syntax",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Theme => "主题包",
            Self::Syntax => "代码高亮扩展包",
        }
    }
}

/// 包的元信息（`pack.toml` 的 `[pack]` 段）。
#[derive(Debug, Clone)]
pub struct PackMeta {
    pub id: String,
    pub kind: PackKind,
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub license: Option<String>,
}

/// 一个已安装的包。
#[derive(Debug, Clone)]
pub struct InstalledPack {
    pub meta: PackMeta,
    pub path: PathBuf,
    /// 安装作用域。
    pub scope: Scope,
}

impl InstalledPack {
    pub fn title(&self) -> String {
        let version = self
            .meta
            .version
            .as_ref()
            .map(|v| format!(" v{v}"))
            .unwrap_or_default();
        format!("{}（{}{}）", self.meta.name, self.meta.id, version)
    }
}

/// 安装作用域。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// 当前用户：`%APPDATA%\coderender`（无需管理员权限）
    User,
    /// 本机所有用户：`%ProgramData%\coderender`（需要管理员权限）
    Machine,
}

impl Scope {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_lowercase().as_str() {
            "user" | "currentuser" | "用户" | "当前用户" => Some(Self::User),
            "machine" | "allusers" | "per-machine" | "permachine" | "机器" | "本机" | "所有用户" => {
                Some(Self::Machine)
            }
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Machine => "machine",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::User => "用户级",
            Self::Machine => "机器级",
        }
    }
}

// ---------------------------------------------------------------- 数据目录

/// 用户数据目录（已安装的包都在这里）。`CODERENDER_HOME` 可覆盖。
pub fn data_dir() -> PathBuf {
    if let Some(override_dir) = std::env::var_os("CODERENDER_HOME") {
        return PathBuf::from(override_dir);
    }
    let base = dirs_base().unwrap_or_else(|| PathBuf::from("."));
    base.join("coderender")
}

/// 机器级数据目录：`%ProgramData%\coderender`（Windows）。
/// 其他平台没有等价约定，返回 `None`（只能装用户级）。
pub fn machine_dir() -> Option<PathBuf> {
    if std::env::var_os("CODERENDER_HOME").is_some() {
        return None; // 测试/便携模式：只有一级目录
    }
    #[cfg(windows)]
    {
        std::env::var_os("ProgramData").map(|dir| PathBuf::from(dir).join("coderender"))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// 某个作用域下某类包的安装根目录。
pub fn packs_dir_in(root: &Path, kind: PackKind) -> PathBuf {
    root.join("packs").join(kind.dir_name())
}

/// 用户级包目录。
pub fn packs_dir(kind: PackKind) -> PathBuf {
    packs_dir_in(&data_dir(), kind)
}

/// 机器级包目录（不支持的平台回落到用户级，调用方应先检查 [`machine_dir`]）。
pub fn machine_packs_dir(kind: PackKind) -> PathBuf {
    match machine_dir() {
        Some(root) => packs_dir_in(&root, kind),
        None => packs_dir(kind),
    }
}

fn dirs_base() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA").map(PathBuf::from)
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join("Library").join("Application Support"))
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("share"))
        })
    }
}

// ---------------------------------------------------------------- 读取

#[derive(Debug, Deserialize)]
struct RawPack {
    pack: RawPackSection,
}

#[derive(Debug, Deserialize)]
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

/// 读取一个开发文件夹（或解包后的目录）里的 `pack.toml`。
pub fn read_meta(dir: &Path) -> Result<PackMeta> {
    let manifest = dir.join(PACK_FILE_NAME);
    let text = fs::read_to_string(&manifest)
        .with_context(|| format!("无法读取 {}（不是有效的扩展包目录？）", manifest.display()))?;
    parse_meta(&text, dir)
        .with_context(|| format!("{} 解析失败（请检查 [pack] 段与字段名）", manifest.display()))
}

fn parse_meta(text: &str, dir: &Path) -> Result<PackMeta> {
    let raw: RawPack = toml::from_str(text)?;
    if raw.pack.id.trim().is_empty() {
        bail!("pack.id 不能为空");
    }
    if !is_valid_id(&raw.pack.id) {
        bail!("pack.id `{}` 只能包含字母、数字、-、_ 和 .", raw.pack.id);
    }
    let kind = match raw.pack.kind.as_deref() {
        Some(kind) => PackKind::parse(kind).ok_or_else(|| {
            anyhow::anyhow!("pack.kind = `{kind}` 无法识别，可用：theme / syntax")
        })?,
        None => detect_kind(text, dir)?,
    };
    Ok(PackMeta {
        id: raw.pack.id.clone(),
        kind,
        name: raw
            .pack
            .name
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(raw.pack.id),
        version: raw.pack.version.filter(|v| !v.trim().is_empty()),
        description: raw.pack.description.filter(|d| !d.trim().is_empty()),
        license: raw.pack.license.filter(|l| !l.trim().is_empty()),
    })
}

fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
}

/// 没写 `pack.kind` 时按内容推断。
fn detect_kind(text: &str, dir: &Path) -> Result<PackKind> {
    if text.contains("[palettes") || text.contains("[[palettes") {
        return Ok(PackKind::Theme);
    }
    if text.contains("[[syntax]]") || text.contains("[[syntaxes]]") {
        return Ok(PackKind::Syntax);
    }
    if dir.join("syntaxes").is_dir() {
        return Ok(PackKind::Syntax);
    }
    if dir.join("palettes").is_dir() {
        return Ok(PackKind::Theme);
    }
    bail!("无法判断扩展包类型：请在 pack.toml 里写 pack.kind = \"theme\" 或 \"syntax\"")
}

/// 列出已安装的包：机器级优先，用户级其次，同 ID 去重。
pub fn installed(kind: PackKind) -> Vec<InstalledPack> {
    let mut packs: Vec<InstalledPack> = Vec::new();

    // 1. 机器级（MSI 装的共享插件）
    if machine_dir().is_some() {
        collect_from(&machine_packs_dir(kind), kind, Scope::Machine, &mut packs);
    }
    // 2. 用户级
    collect_from(&packs_dir(kind), kind, Scope::User, &mut packs);

    packs.sort_by(|a, b| a.meta.id.cmp(&b.meta.id));
    packs.dedup_by(|a, b| a.meta.id == b.meta.id);
    packs
}

fn collect_from(dir: &Path, kind: PackKind, scope: Scope, out: &mut Vec<InstalledPack>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if let Ok(meta) = read_meta(&path) {
            if meta.kind == kind {
                out.push(InstalledPack {
                    meta,
                    path,
                    scope,
                });
            }
        }
    }
}

pub fn installed_ids(kind: PackKind) -> Vec<String> {
    installed(kind).into_iter().map(|p| p.meta.id).collect()
}

pub fn find_installed(kind: PackKind, id: &str) -> Option<InstalledPack> {
    installed(kind).into_iter().find(|pack| {
        pack.meta.id == id
            || fold(&pack.meta.id) == fold(id)
            || fold(&pack.meta.name) == fold(id)
    })
}

fn fold(text: &str) -> String {
    text.chars()
        .filter(|ch| ch.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

// ---------------------------------------------------------------- 安装 / 卸载

/// 安装一个扩展包：目录（开发文件夹）或 `.crpack`（分发格式）。
///
/// `scope = Machine` 时装到 `%ProgramData%\coderender`（需要管理员权限）。
pub fn install(path: &Path, scope: Scope) -> Result<PackMeta> {
    if !path.exists() {
        bail!("找不到要安装的路径 {}", path.display());
    }
    if scope == Scope::Machine && machine_dir().is_none() {
        bail!("当前平台没有机器级安装目录（只有 Windows 的 %ProgramData%\\coderender），请用 --scope user");
    }

    let mut temp: Option<PathBuf> = None;
    let source = if is_distribution(path) {
        let unpacked = data_dir().join(".unpacking");
        let _ = fs::remove_dir_all(&unpacked);
        fs::create_dir_all(&unpacked)
            .with_context(|| format!("无法创建临时目录 {}", unpacked.display()))?;
        extract_distribution(path, &unpacked)?;
        let root = single_subdirectory(&unpacked).unwrap_or(unpacked.clone());
        temp = Some(unpacked);
        root
    } else if path.is_dir() {
        path.to_path_buf()
    } else {
        bail!(
            "无法识别的扩展包文件 `{}`（支持开发目录或 .{PACK_EXTENSION} 分发包）",
            path.display()
        );
    };

    let result = install_from_dir(&source, scope);
    if let Some(temp) = temp {
        let _ = fs::remove_dir_all(temp);
    }
    result
}

fn install_from_dir(source: &Path, scope: Scope) -> Result<PackMeta> {
    let meta = read_meta(source)?;
    let root = match scope {
        Scope::User => data_dir(),
        Scope::Machine => machine_dir().unwrap_or_else(data_dir),
    };
    let dest = packs_dir_in(&root, meta.kind).join(&meta.id);
    if dest.exists() {
        fs::remove_dir_all(&dest)
            .with_context(|| format!("无法覆盖已安装的包 {}", dest.display()))?;
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("无法创建目录 {}（机器级安装需要管理员权限）", parent.display()))?;
    }
    copy_dir_all(source, &dest).with_context(|| {
        format!("复制 {} → {} 失败", source.display(), dest.display())
    })?;
    Ok(meta)
}

pub fn uninstall(kind: PackKind, id: &str) -> Result<PackMeta> {
    let Some(pack) = find_installed(kind, id) else {
        let available = installed_ids(kind);
        bail!(
            "没有安装过{} `{}`{}。可用 `coderender pack list` 查看已安装的包。",
            kind.label(),
            id,
            dym::did_you_mean(id, &available, 3)
        );
    };
    fs::remove_dir_all(&pack.path)
        .with_context(|| format!("无法删除 {}", pack.path.display()))?;
    Ok(pack.meta)
}

fn copy_dir_all(source: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let path = entry.path();
        let target = dest.join(entry.file_name());
        if path.is_dir() {
            copy_dir_all(&path, &target)?;
        } else {
            fs::copy(&path, &target)
                .with_context(|| format!("复制 {} 失败", path.display()))?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- 分发格式

/// 判断是否为分发包：扩展名 `.crpack` 或 zip 魔数。
pub fn is_distribution(path: &Path) -> bool {
    if path
        .extension()
        .map(|ext| ext.eq_ignore_ascii_case(PACK_EXTENSION))
        .unwrap_or(false)
    {
        return true;
    }
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let mut magic = [0u8; 4];
    let read = file.read(&mut magic).unwrap_or(0);
    read == 4 && &magic[..2] == b"PK"
}

/// 把开发文件夹打包成 `.crpack`。
pub fn build_distribution(dir: &Path, output: Option<&Path>) -> Result<PathBuf> {
    let meta = read_meta(dir)?;
    if !dir.is_dir() {
        bail!("{} 不是目录", dir.display());
    }

    let out = match output {
        Some(path) => path.to_path_buf(),
        None => PathBuf::from(format!(
            "{}-{}.{}",
            meta.id,
            meta.version.as_deref().unwrap_or("0.0.0"),
            PACK_EXTENSION
        )),
    };
    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .with_context(|| format!("无法创建目录 {}", parent.display()))?;
        }
    }

    let file =
        fs::File::create(&out).with_context(|| format!("无法创建 {}", out.display()))?;
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    // pack.toml 放最前面，方便人直接看 zip 内容
    write_file(&mut writer, options, dir, &dir.join(PACK_FILE_NAME), PACK_FILE_NAME)?;
    write_tree(&mut writer, options, dir, dir)?;
    writer.finish().context("写入分发包失败")?;

    Ok(out)
}

fn write_tree(
    writer: &mut zip::ZipWriter<fs::File>,
    options: zip::write::SimpleFileOptions,
    root: &Path,
    dir: &Path,
) -> Result<()> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)?
        .flatten()
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    for path in entries {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        // pack.toml 已经第一个写入，跳过避免重复条目
        if name == PACK_FILE_NAME && dir == root {
            continue;
        }
        if name == ".DS_Store" || (name.starts_with('.') && path.is_file()) {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if path.is_dir() {
            writer
                .add_directory(format!("{relative}/"), options)
                .with_context(|| format!("写入目录 {relative} 失败"))?;
            write_tree(writer, options, root, &path)?;
        } else {
            write_file(writer, options, root, &path, &relative)?;
        }
    }
    Ok(())
}

fn write_file(
    writer: &mut zip::ZipWriter<fs::File>,
    options: zip::write::SimpleFileOptions,
    root: &Path,
    path: &Path,
    relative: &str,
) -> Result<()> {
    let Ok(relative_path) = path.strip_prefix(root) else {
        return Ok(());
    };
    if relative_path.as_os_str().is_empty() {
        return Ok(());
    }
    let data = fs::read(path).with_context(|| format!("读取 {} 失败", path.display()))?;
    writer
        .start_file(relative, options)
        .with_context(|| format!("写入 {relative} 失败"))?;
    writer.write_all(&data)?;
    Ok(())
}

/// 解压分发包（带路径逃逸保护）。
pub fn extract_distribution(file: &Path, dest: &Path) -> Result<()> {
    let reader =
        fs::File::open(file).with_context(|| format!("无法打开 {}", file.display()))?;
    let mut archive = zip::ZipArchive::new(reader)
        .with_context(|| format!("{} 不是有效的 {} 包", file.display(), PACK_EXTENSION))?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let Some(name) = entry.enclosed_name() else {
            continue;
        };
        let target = dest.join(name);
        if entry.is_dir() {
            fs::create_dir_all(&target)?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut out = fs::File::create(&target)
                .with_context(|| format!("无法创建 {}", target.display()))?;
            std::io::copy(&mut entry, &mut out)?;
        }
    }
    Ok(())
}

pub fn single_subdirectory(dir: &Path) -> Option<PathBuf> {
    let entries: Vec<PathBuf> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .map(|n| !n.to_string_lossy().starts_with('.'))
                .unwrap_or(false)
        })
        .collect();
    if entries.len() == 1 && entries[0].is_dir() {
        Some(entries[0].clone())
    } else {
        None
    }
}

// ---------------------------------------------------------------- 开发模板

/// 生成一个开发模板目录，返回目录路径。
pub fn create_template(name: &str, kind: PackKind, output: Option<&Path>) -> Result<PathBuf> {
    let id = slug(name);
    if id.is_empty() {
        bail!("包名不能为空");
    }
    let dir = match output {
        Some(path) => path.to_path_buf(),
        None => PathBuf::from(&id),
    };
    if dir.exists() {
        bail!("目录 {} 已存在", dir.display());
    }
    fs::create_dir_all(&dir).with_context(|| format!("无法创建 {}", dir.display()))?;

    match kind {
        PackKind::Theme => write_theme_template(&dir, &id, name)?,
        PackKind::Syntax => write_syntax_template(&dir, &id, name)?,
    }
    Ok(dir)
}

fn slug(name: &str) -> String {
    let mut out = String::new();
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if (ch == '-' || ch == '_' || ch == ' ') && !out.ends_with('-') && !out.is_empty()
        {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn write_theme_template(dir: &Path, id: &str, name: &str) -> Result<()> {
    let manifest = format!(
        r##"# {name} —— coderender 主题包
# 安装：coderender pack install {id}
# 打包：coderender pack build {id}

[pack]
id = "{id}"
kind = "theme"
name = "{name}"
version = "1.0.0"
description = "我的主题包"
license = "MIT"

# ---- 代码配色预设：每套配色自带代码背景色 ----
[palettes.my-style]
name = "My Style"
appearance = "dark"          # dark | light
background = "#1e1e2e"       # 代码背景
foreground = "#cdd6f4"
titlebar = "#181825"         # 可选，缺省从背景/前景推导
border = "#313244"           # 可选
gutter = "#6c7086"           # 可选，行号颜色

[palettes.my-style.syntax]
keyword = "#cba6f7"
function = "#89b4fa"
type = "#f9e2af"
string = "#a6e3a1"
number = "#fab387"
comment = "#7f849c"
operator = "#89dceb"
punctuation = "#9399b2"

# ---- 窗口装饰预设 ----
[decorations.default]
title_height = 36            # 0 = 无标题栏
corner_radius = 12
title_align = "center"       # left | center
padding = 28

[decorations.default.shadow]
offset_y = 14
blur = 30
alpha = 0.42

[decorations.default.controls]
style = "traffic_lights"     # traffic_lights | windows | none
radius = 6.5
spacing = 19
inset = 20
close = "#ff5f57"
minimize = "#febc2e"
maximize = "#28c840"
"##
    );
    fs::write(dir.join(PACK_FILE_NAME), manifest)?;
    Ok(())
}

fn write_syntax_template(dir: &Path, id: &str, name: &str) -> Result<()> {
    fs::create_dir_all(dir.join("syntaxes"))?;
    let manifest = format!(
        r##"# {name} —— coderender 代码高亮扩展包
# 安装：coderender pack install {id}
# 打包：coderender pack build {id}

[pack]
id = "{id}"
kind = "syntax"
name = "{name}"
version = "1.0.0"
description = "我的语言语法"
license = "MIT"

# 显式声明要加载的语法文件（相对包根目录）。
# 也可以省略本段，程序会自动扫描 syntaxes/ 目录下的
# *.sublime-syntax 与 *.tmLanguage 文件。
[[syntax]]
path = "syntaxes/MyLang.sublime-syntax"
"##
    );
    fs::write(dir.join(PACK_FILE_NAME), manifest)?;

    let syntax = r#"# 最小的 Sublime Syntax 定义：把注释、关键字、字符串、数字上色。
# 语法规范见 https://www.sublimetext.com/docs/syntax.html
%YAML 1.2
---
name: MyLang
file_extensions: [mylang]
scope: source.mylang

contexts:
  main:
    - match: '#.*$'
      scope: comment.line.number-sign.mylang
    - match: '"(?:\\.|[^"])*"'
      scope: string.quoted.double.mylang
    - match: '\b\d+\b'
      scope: constant.numeric.mylang
    - match: '\b(if|else|while|return|fn|let)\b'
      scope: keyword.control.mylang
"#;
    fs::write(
        dir.join("syntaxes").join("MyLang.sublime-syntax"),
        syntax,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `CODERENDER_HOME` 是进程级环境变量，测试里改它必须串行。
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "coderender-pack-test-{}-{tag}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn slug_normalizes_names() {
        assert_eq!(slug("My Syntax Pack!"), "my-syntax-pack");
        assert_eq!(slug("  Zig  "), "zig");
    }

    #[test]
    fn parses_meta_with_explicit_kind() {        let dir = temp("meta");
        fs::write(
            dir.join(PACK_FILE_NAME),
            r#"
[pack]
id = "demo"
kind = "syntax"
name = "Demo"
version = "1.2.3"
"#,
        )
        .unwrap();
        let meta = read_meta(&dir).unwrap();
        assert_eq!(meta.kind, PackKind::Syntax);
        assert_eq!(meta.version.as_deref(), Some("1.2.3"));
        assert_eq!(meta.name, "Demo");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detects_kind_from_content() {
        let dir = temp("detect");
        fs::write(
            dir.join(PACK_FILE_NAME),
            "[pack]\nid = \"x\"\n\n[palettes.p]\nbackground = \"#000\"\nforeground = \"#fff\"\n",
        )
        .unwrap();
        assert_eq!(read_meta(&dir).unwrap().kind, PackKind::Theme);

        let dir2 = temp("detect2");
        fs::write(dir2.join(PACK_FILE_NAME), "[pack]\nid = \"y\"\n").unwrap();
        fs::create_dir_all(dir2.join("syntaxes")).unwrap();
        assert_eq!(read_meta(&dir2).unwrap().kind, PackKind::Syntax);
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&dir2);
    }

    #[test]
    fn machine_scope_is_disabled_in_portable_mode() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let dir = temp("portable");
        let home = dir.join("home");
        std::env::set_var("CODERENDER_HOME", &home);
        // 便携/测试模式下只有一级目录，机器级安装应被拒绝
        assert!(machine_dir().is_none());
        assert!(install(&dir, Scope::Machine).is_err());
        std::env::remove_var("CODERENDER_HOME");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn installed_only_scans_user_dir_in_portable_mode() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let dir = temp("userscope");
        let home = dir.join("home");
        std::env::set_var("CODERENDER_HOME", &home);

        let source = dir.join("src");
        fs::create_dir_all(&source).unwrap();
        fs::write(
            source.join(PACK_FILE_NAME),
            "[pack]\nid = \"scoped\"\nkind = \"theme\"\n\n[palettes.a]\nbackground = \"#000\"\nforeground = \"#fff\"\n",
        )
        .unwrap();
        let meta = install(&source, Scope::User).unwrap();
        assert_eq!(meta.id, "scoped");

        let packs = installed(PackKind::Theme);
        assert_eq!(packs.len(), 1);
        assert_eq!(packs[0].scope, Scope::User);
        assert!(packs[0].path.starts_with(&home));

        std::env::remove_var("CODERENDER_HOME");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_scope_names() {
        assert_eq!(Scope::parse("user"), Some(Scope::User));
        assert_eq!(Scope::parse("MACHINE"), Some(Scope::Machine));
        assert_eq!(Scope::parse("所有用户"), Some(Scope::Machine));
        assert_eq!(Scope::parse("nope"), None);
    }

    #[test]
    fn rejects_bad_id() {
        let dir = temp("badid");
        fs::write(
            dir.join(PACK_FILE_NAME),
            "[pack]\nid = \"a b\"\nkind = \"theme\"\n",
        )
        .unwrap();
        assert!(read_meta(&dir).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn distribution_roundtrip() {
        let dir = temp("dist");
        let source = dir.join("my-theme");
        fs::create_dir_all(&source).unwrap();
        fs::write(
            source.join(PACK_FILE_NAME),
            "[pack]\nid = \"my-theme\"\nkind = \"theme\"\nversion = \"9.9.9\"\n\n[palettes.p]\nbackground = \"#000000\"\nforeground = \"#ffffff\"\n",
        )
        .unwrap();
        fs::write(source.join("notes.txt"), "hello").unwrap();

        let out = build_distribution(&source, Some(&dir.join("my-theme.crpack"))).unwrap();
        assert!(out.exists());
        assert!(is_distribution(&out));

        let extracted = dir.join("extracted");
        fs::create_dir_all(&extracted).unwrap();
        extract_distribution(&out, &extracted).unwrap();
        assert_eq!(
            fs::read_to_string(extracted.join(PACK_FILE_NAME)).unwrap(),
            fs::read_to_string(source.join(PACK_FILE_NAME)).unwrap()
        );
        assert_eq!(
            fs::read_to_string(extracted.join("notes.txt")).unwrap(),
            "hello"
        );

        let home = dir.join("home");
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        std::env::set_var("CODERENDER_HOME", &home);
        let meta = install(&out, Scope::User).unwrap();
        assert_eq!(meta.kind, PackKind::Theme);
        let installed_list = installed(PackKind::Theme);
        assert_eq!(installed_list.len(), 1);
        assert_eq!(installed_list[0].meta.id, "my-theme");
        assert!(installed_list[0].path.join(PACK_FILE_NAME).exists());

        uninstall(PackKind::Theme, "my-theme").unwrap();
        assert!(installed(PackKind::Theme).is_empty());
        std::env::remove_var("CODERENDER_HOME");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_from_development_folder() {
        let dir = temp("devfolder");
        let source = dir.join("zig-extra");
        fs::create_dir_all(source.join("syntaxes")).unwrap();
        fs::write(
            source.join(PACK_FILE_NAME),
            "[pack]\nid = \"zig-extra\"\nkind = \"syntax\"\n",
        )
        .unwrap();
        fs::write(
            source.join("syntaxes").join("Zig.sublime-syntax"),
            "name: Zig\n",
        )
        .unwrap();

        let home = dir.join("home");
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        std::env::set_var("CODERENDER_HOME", &home);
        let meta = install(&source, Scope::User).unwrap();
        assert_eq!(meta.kind, PackKind::Syntax);
        let pack = find_installed(PackKind::Syntax, "zig-extra").unwrap();
        assert!(pack.path.join("syntaxes").join("Zig.sublime-syntax").exists());

        let missing = uninstall(PackKind::Syntax, "zig-xtra")
            .unwrap_err()
            .to_string();
        assert!(missing.contains("zig-extra"), "{missing}");
        assert!(missing.contains("您是否指的是"), "{missing}");
        std::env::remove_var("CODERENDER_HOME");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn creates_templates() {
        let dir = temp("template");
        for kind in PackKind::ALL {
            let target = dir.join(kind.dir_name());
            let created = create_template("My Pack", kind, Some(&target)).unwrap();
            assert!(created.join(PACK_FILE_NAME).exists());
            assert_eq!(read_meta(&created).unwrap().kind, kind);
        }
        let syntax = dir
            .join("syntax")
            .join("syntaxes")
            .join("MyLang.sublime-syntax");
        assert!(syntax.exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
