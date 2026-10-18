//! 构建脚本：默认什么都不做；开启 `msi` feature 时额外生成 Windows 安装包。
//!
//! ```bash
//! cargo build --release --features msi     # 产出 target/msi/coderender-<版本>-x64.msi
//! ```
//!
//! 依赖 WiX 工具链（任选其一）：
//! * WiX 4 CLI：`dotnet tool install --global wix`，之后 `wix` 在 PATH 里；
//! * WiX 4 本地工具：`dotnet tool install --local wix`（设 `CODERENDER_WIX` 指向 `wix.exe`）；
//! * WiX 3：把 `candle.exe` / `light.exe` 所在目录设到 `CODERENDER_WIX`。
//!
//! 只想检查生成的 WiX 源码、不真正打包：
//! `CODERENDER_MSI_WXS_ONLY=1 cargo build --release --features msi`
//! （产物：`target/msi/coderender.wxs` + `target/msi/stage/`）

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=packaging/wix/main.wxs.in");
    println!("cargo:rerun-if-changed=assets");
    println!("cargo:rerun-if-changed=README.md");
    println!("cargo:rerun-if-env-changed=CODERENDER_WIX");
    println!("cargo:rerun-if-env-changed=CODERENDER_MSI_WXS_ONLY");

    // feature 通过 CARGO_FEATURE_<NAME> 暴露给 build.rs
    if std::env::var_os("CARGO_FEATURE_MSI").is_none() {
        return;
    }

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        println!("cargo:warning=msi feature 只在 Windows 上有意义，已跳过");
        return;
    }

    if let Err(error) = build_msi() {
        // 用户显式要求打 MSI 时，缺工具链/未接受协议都应该让构建失败并给出可操作提示
        if error.downcast_ref::<NeedsEula>().is_some() {
            panic!("生成 MSI 失败：{error:#}");
        }
        panic!(
            "生成 MSI 失败：{error:#}\n\n请先安装 WiX：dotnet tool install --global wix\n\
             或参考 packaging/build-msi.ps1。"
        );
    }
}

// ---------------------------------------------------------------- 主流程

fn build_msi() -> anyhow::Result<()> {
    let exe = binary_path()?;
    // 产物放在「二进制所在目录」下的 msi/ 子目录：
    // target/release/msi/... 或 target/<triple>/release/msi/...
    let msi_dir = exe
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("msi");
    std::fs::create_dir_all(&msi_dir)?;

    let version = std::env::var("CARGO_PKG_VERSION")?;
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH")?; // x86_64 | aarch64
    let wxs_arch = match arch.as_str() {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "x86",
        other => anyhow::bail!("不支持的目标架构 `{other}`（支持 x86_64 / aarch64 / x86）"),
    };

    // 1) 暂存要安装的文件（exe + 许可证/说明）
    let stage = msi_dir.join("stage");
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage)?;
    std::fs::copy(&exe, stage.join(exe.file_name().unwrap()))?;
    for extra in ["README.md", "assets/fonts/OFL.txt"] {
        let source = workspace_root().join(extra);
        if source.is_file() {
            let name = Path::new(extra).file_name().unwrap().to_os_string();
            std::fs::copy(&source, stage.join(name))?;
        }
    }

    // 2) 生成 WiX 源码
    let wxs = msi_dir.join("coderender.wxs");
    let manifest = std::fs::read_to_string(workspace_root().join("Cargo.toml"))?;
    let upgrade_code = metadata_value(&manifest, "upgrade-code")
        // 兜底值与 Cargo.toml 的 [package.metadata.msi] 保持一致；
        // 升级 GUID 必须跨版本稳定，只有换产品时才生成新的。
        .unwrap_or_else(|| "3cdd2adc-5609-4e3d-b36d-763724e89023".to_string());
    let manufacturer =
        metadata_value(&manifest, "manufacturer").unwrap_or_else(|| "coderender".to_string());
    let template = std::fs::read_to_string(workspace_root().join("packaging/wix/main.wxs.in"))?;
    validate_template(&template)?;
    let wxs_text = expand_template(
        template,
        &TemplateValues {
            version: &version,
            manufacturer: &manufacturer,
            upgrade_code: &upgrade_code,
            arch: wxs_arch,
            install_dirs: &render_install_tree(&stage)?,
            packs_dirs: &render_packs_dirs(),
            feature_components: &render_feature_components(&stage)?,
            path_guid: &stable_guid("coderender/path-env"),
        },
    );
    std::fs::write(&wxs, &wxs_text)?;
    validate_wxs(&wxs_text)?;
    println!("cargo:warning=已生成 WiX 源码 {}", wxs.display());

    // 3) 只生成源码不打包（CI 检查 / 手工调整时用）
    if std::env::var_os("CODERENDER_MSI_WXS_ONLY").is_some() {
        println!("cargo:warning=CODERENDER_MSI_WXS_ONLY=1，跳过 WiX 调用");
        return Ok(());
    }

    // 4) 调用 WiX
    let msi = msi_dir.join(format!("coderender-{version}-{wxs_arch}.msi"));
    // 清掉上一次构建遗留的外置 cabinet（EmbedCab=yes 后不会再生成，
    // 但旧文件留在目录里容易让人误以为要一起分发）
    if let Ok(entries) = std::fs::read_dir(&msi_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_cab = path
                .extension()
                .map(|ext| ext.eq_ignore_ascii_case("cab"))
                .unwrap_or(false);
            if is_cab {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    run_wix(&wxs, &msi, &stage, wxs_arch)?;
    println!(
        "cargo:warning=安装包已生成：{}（{:.1} MB）",
        msi.display(),
        std::fs::metadata(&msi)?.len() as f64 / 1e6
    );
    Ok(())
}

// ---------------------------------------------------------------- 路径

fn workspace_root() -> PathBuf {
    PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap())
}

/// 找到已编译好的 `coderender.exe`。
///
/// build.rs 在链接前执行，所以第一次 `cargo build --features msi` 时二进制还不存在；
/// 这时会给出「再跑一次」的提示。找到后 MSI 产物放在二进制同级的 `msi/` 目录。
fn binary_path() -> anyhow::Result<PathBuf> {
    let name = format!("coderender{}", std::env::consts::EXE_SUFFIX);

    // 1) 从 OUT_DIR 往上找（target/release、target/<triple>/release 都能命中）
    if let Some(out_dir) = std::env::var_os("OUT_DIR") {
        for ancestor in PathBuf::from(out_dir).ancestors() {
            let candidate = ancestor.join(&name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    // 2) 兜底：按 profile 拼常见位置
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "release".to_string());
    for base in [
        workspace_root().join("target"),
        workspace_root().join("target").join("release"),
        workspace_root().join("target").join(&profile),
    ] {
        let candidate = base.join(&name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    anyhow::bail!(
        "找不到可执行文件 `{name}`。build.rs 在链接前执行：第一次 `cargo build --features msi` \
         只会生成 WiX 源码，请再执行一次同一条命令即可打出 MSI。"
    )
}

// ---------------------------------------------------------------- WiX 源码生成

struct TemplateValues<'a> {
    version: &'a str,
    manufacturer: &'a str,
    upgrade_code: &'a str,
    arch: &'a str,
    install_dirs: &'a str,
    packs_dirs: &'a str,
    feature_components: &'a str,
    path_guid: &'a str,
}

fn expand_template(template: String, values: &TemplateValues<'_>) -> String {
    template
        .replace("@VERSION@", values.version)
        .replace("@MANUFACTURER@", values.manufacturer)
        .replace("@UPGRADE_CODE@", values.upgrade_code)
        .replace("@ARCH@", values.arch)
        .replace("@INSTALL_DIRS@", values.install_dirs)
        .replace("@PACKS_DIRS@", values.packs_dirs)
        .replace("@FEATURE_COMPONENTS@", values.feature_components)
        .replace("@PATH_GUID@", values.path_guid)
}

/// 把暂存目录渲染成嵌套的 `<Directory>`/`<Component>` 树。
///
/// 每个含文件的目录一个 Component（GUID 由路径稳定派生，重复构建不变），
/// 这样不依赖 `heat.exe` 收割，也不依赖 `Guid="*"`。
fn render_install_tree(stage: &Path) -> anyhow::Result<String> {
    let mut files: Vec<String> = Vec::new();
    collect_files(stage, stage, &mut files)?;
    files.sort();
    let mut out = String::new();
    emit_tree(stage, "", &files, 1, &mut out);
    Ok(out)
}

fn parent_of(file: &str) -> &str {
    file.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("")
}

fn emit_tree(stage: &Path, dir: &str, files: &[String], depth: usize, out: &mut String) {
    let indent = " ".repeat(6 + depth * 2);

    // 本目录下的文件
    let here: Vec<&String> = files.iter().filter(|file| parent_of(file) == dir).collect();
    if !here.is_empty() {
        let (id, directory) = if dir.is_empty() {
            ("CmpRoot".to_string(), None)
        } else {
            (format!("Cmp{}", sanitize_id(dir)), Some(dir_id(dir)))
        };
        let xml: Vec<String> = here.iter().map(|file| file_xml(stage, file)).collect();
        out.push_str(&component(
            &id,
            &stable_guid(&format!("coderender/component/{dir}")),
            directory.as_deref(),
            &xml,
            6 + depth * 2,
        ));
        out.push('\n');
    }

    // 直接子目录
    for sub in immediate_subdirs(dir, files) {
        let name = sub.rsplit('/').next().unwrap_or(&sub);
        out.push_str(&format!(
            "{indent}<Directory Id=\"{}\" Name=\"{name}\">\n",
            dir_id(&sub)
        ));
        emit_tree(stage, &sub, files, depth + 1, out);
        out.push_str(&format!("{indent}</Directory>\n"));
    }
}

/// `dir` 下直接包含文件的子目录（相对路径，"/" 分隔）。
fn immediate_subdirs(dir: &str, files: &[String]) -> Vec<String> {
    let mut subdirs: Vec<String> = Vec::new();
    for file in files {
        let parent = parent_of(file);
        if parent == dir {
            continue;
        }
        let relative = if dir.is_empty() {
            parent.to_string()
        } else if let Some(rest) = parent.strip_prefix(&format!("{dir}/")) {
            rest.to_string()
        } else {
            continue;
        };
        if let Some((first, _)) = relative.split_once('/') {
            let child = if dir.is_empty() {
                first.to_string()
            } else {
                format!("{dir}/{first}")
            };
            if !subdirs.contains(&child) {
                subdirs.push(child);
            }
        }
    }
    subdirs
}

fn component(
    id: &str,
    guid: &str,
    directory: Option<&str>,
    files: &[String],
    indent: usize,
) -> String {
    let pad = " ".repeat(indent);
    let dir_attr = directory
        .map(|dir| format!(" Directory=\"{dir}\""))
        .unwrap_or_default();
    let mut out = format!("{pad}<Component Id=\"{id}\" Guid=\"{guid}\"{dir_attr}>\n");
    for file in files {
        out.push_str(&format!("{pad}  {file}\n"));
    }
    out.push_str(&format!("{pad}</Component>"));
    out
}

fn file_xml(stage: &Path, relative: &str) -> String {
    let source = stage.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
    format!(
        "<File Id=\"{}\" Source=\"{}\" />",
        file_id(relative),
        xml_escape(&source.display().to_string())
    )
}

fn file_id(relative: &str) -> String {
    format!("F{}", sanitize_id(relative))
}

fn dir_id(dir: &str) -> String {
    format!("D{}", sanitize_id(dir))
}

fn sanitize_id(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        out.insert(0, 'x');
    }
    out
}

/// 机器级扩展包目录（MSI 只负责创建空目录，实际插件由管理员投放）。
///
/// 用一个组件负责整棵 `%ProgramData%\coderender\packs` 树：先创建，
/// 卸载时按「先深后浅」的顺序 RemoveFolder，避免残留空的父目录。
fn render_packs_dirs() -> String {
    let pad = " ".repeat(8);
    let guid = stable_guid("coderender/packs");
    // 注意：不要在这里写 XML 注释，也不要在模板里把占位符放进注释中，
    // 否则展开后的内容会破坏注释结构。
    format!(
        "{pad}<Component Id=\"CmpPacks\" Directory=\"PACKSROOT\" Guid=\"{guid}\">\n\
         {pad}  <CreateFolder />\n\
         {pad}  <CreateFolder Directory=\"Packs_themes\" />\n\
         {pad}  <CreateFolder Directory=\"Packs_syntax\" />\n\
         {pad}  <RemoveFolder Id=\"RemovePacksSyntax\" On=\"uninstall\" Directory=\"Packs_syntax\" />\n\
         {pad}  <RemoveFolder Id=\"RemovePacksThemes\" On=\"uninstall\" Directory=\"Packs_themes\" />\n\
         {pad}  <RemoveFolder Id=\"RemovePacksRoot\" On=\"uninstall\" Directory=\"PACKSROOT\" />\n\
         {pad}  <RemoveFolder Id=\"RemoveCoderenderData\" On=\"uninstall\" Directory=\"CODERENDERDATA\" />\n\
         {pad}</Component>\n",
        pad = pad
    )
}

/// Feature 里对 Directory 树中所有组件的引用。
fn render_feature_components(stage: &Path) -> anyhow::Result<String> {
    let mut files: Vec<String> = Vec::new();
    collect_files(stage, stage, &mut files)?;
    files.sort();

    let mut ids: Vec<String> = Vec::new();
    collect_component_ids("", &files, &mut ids);
    ids.push("CmpPacks".to_string());

    let pad = " ".repeat(6);
    let mut out = ids
        .iter()
        .map(|id| format!("{pad}<ComponentRef Id=\"{id}\" />"))
        .collect::<Vec<_>>()
        .join("\n");
    out.push('\n');
    Ok(out)
}

fn collect_component_ids(dir: &str, files: &[String], out: &mut Vec<String>) {
    if files.iter().any(|file| parent_of(file) == dir) {
        out.push(if dir.is_empty() {
            "CmpRoot".to_string()
        } else {
            format!("Cmp{}", sanitize_id(dir))
        });
    }
    for sub in immediate_subdirs(dir, files) {
        collect_component_ids(&sub, files, out);
    }
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<String>) -> anyhow::Result<()> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?
        .flatten()
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_files(root, &path, out)?;
        } else {
            let relative = path
                .strip_prefix(root)?
                .components()
                .map(|component| component.as_os_str().to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("/");
            out.push(relative);
        }
    }
    Ok(())
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// 由名字稳定派生 GUID（FNV-1a 双哈希），保证同一路径每次构建得到同一个 GUID。
fn stable_guid(seed: &str) -> String {
    let mut h1: u64 = 0xcbf2_9ce4_8422_2325;
    let mut h2: u64 = 0x9e37_79b9_7f4a_7c15;
    for byte in seed.as_bytes() {
        h1 = (h1 ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
        h2 = (h2.rotate_left(31) ^ u64::from(*byte)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:04X}-{:012X}}}",
        (h1 >> 32) as u32,
        ((h1 >> 16) & 0xffff) as u16,
        (h1 & 0xffff) as u16,
        ((h2 >> 48) & 0xffff) as u16,
        h2 & 0x0000_ffff_ffff_ffff
    )
}

/// 校验模板本身：占位符不能写在 XML 注释里（否则展开后的内容会破坏注释结构），
/// 注释里也不能有连续减号。
fn validate_template(template: &str) -> anyhow::Result<()> {
    let mut in_comment = false;
    for (index, line) in template.lines().enumerate() {
        let number = index + 1;
        if in_comment {
            if let Some(end) = line.find("-->") {
                check_comment_body(&line[..end], number)?;
                check_placeholder(&line[..end], number)?;
                in_comment = false;
            } else {
                check_comment_body(line, number)?;
                check_placeholder(line, number)?;
            }
        } else if let Some(start) = line.find("<!--") {
            let after = &line[start + 4..];
            match after.find("-->") {
                Some(end) => {
                    check_comment_body(&after[..end], number)?;
                    check_placeholder(&after[..end], number)?;
                }
                None => {
                    check_comment_body(after, number)?;
                    check_placeholder(after, number)?;
                    in_comment = true;
                }
            }
        }
    }
    Ok(())
}

/// 注释里不允许出现 `@NAME@` 形式的占位符。
fn check_placeholder(text: &str, line_number: usize) -> anyhow::Result<()> {
    if let Some(at) = text.find('@') {
        let tail = &text[at + 1..];
        if let Some(end) = tail.find('@') {
            let inner = &tail[..end];
            if !inner.is_empty() && inner.chars().all(|ch| ch.is_ascii_uppercase() || ch == '_') {
                anyhow::bail!(
                    "模板第 {line_number} 行的 XML 注释里含有占位符 @{inner}@，\
                     展开后会破坏注释结构，请把占位符移到注释外面"
                );
            }
        }
    }
    Ok(())
}

/// 对生成的 WiX 源码做最基本的自检：占位符要替换干净、XML 注释不能含 `--`、
/// 关键标签要配对。
fn validate_wxs(text: &str) -> anyhow::Result<()> {
    let mut in_comment = false;

    for (index, line) in text.lines().enumerate() {
        let number = index + 1;

        // 1) @XXX@ 占位符必须全部替换
        let mut rest = line;
        while let Some(at) = rest.find('@') {
            let tail = &rest[at + 1..];
            let Some(end) = tail.find('@') else { break };
            let inner = &tail[..end];
            if !inner.is_empty() && inner.chars().all(|ch| ch.is_ascii_uppercase() || ch == '_') {
                anyhow::bail!(
                    "生成的 WiX 源码第 {number} 行有未替换的占位符 @{inner}@：{}",
                    line.trim()
                );
            }
            rest = &tail[end + 1..];
        }

        // 2) XML 注释（可能跨行）里不能有连续减号
        if in_comment {
            if let Some(end) = line.find("-->") {
                check_comment_body(&line[..end], number)?;
                in_comment = false;
            } else {
                check_comment_body(line, number)?;
            }
        } else if let Some(start) = line.find("<!--") {
            let after = &line[start + 4..];
            match after.find("-->") {
                Some(end) => check_comment_body(&after[..end], number)?,
                None => {
                    check_comment_body(after, number)?;
                    in_comment = true;
                }
            }
        }
    }

    if in_comment {
        anyhow::bail!("生成的 WiX 源码里有未闭合的 XML 注释");
    }
    if !text.contains("<Wix ") || !text.contains("</Wix>") {
        anyhow::bail!("生成的 WiX 源码缺少 <Wix> 根元素");
    }
    // 关键标签配对（防止展开结果把结构冲掉）。自闭合标签不计入。
    for tag in ["Wix", "Package", "Feature", "StandardDirectory", "Directory", "Component"] {
        let (open, close) = tag_balance(text, tag);
        if open != close {
            anyhow::bail!(
                "生成的 WiX 源码标签不配对：<{tag}> 开 {open} 个 / 闭 {close} 个"
            );
        }
    }
    Ok(())
}

/// 统计一对标签的非自闭合开标签数与闭标签数。
fn tag_balance(text: &str, tag: &str) -> (usize, usize) {
    let needle = format!("<{tag}");
    let mut open = 0usize;
    let mut cursor = 0usize;
    while let Some(found) = text[cursor..].find(&needle) {
        let start = cursor + found + needle.len();
        let next = text[start..].chars().next();
        // 只接受 `<tag `、`<tag>`、`<tag\n` 这类，避免匹配到 <tagFoo
        if matches!(next, Some(' ') | Some('>') | Some('\t') | Some('\n')) {
            if let Some(gt) = text[start..].find('>') {
                let body = &text[start..start + gt];
                if !body.trim_end().ends_with('/') {
                    open += 1;
                }
            }
        }
        cursor = start;
    }
    let close = text.matches(&format!("</{tag}>")).count();
    (open, close)
}

fn check_comment_body(body: &str, line_number: usize) -> anyhow::Result<()> {
    if body.contains("--") {
        anyhow::bail!(
            "生成的 WiX 源码第 {line_number} 行的 XML 注释含有连续减号（XML 不允许）：{}",
            body.trim()
        );
    }
    Ok(())
}

// ---------------------------------------------------------------- 调用 WiX

fn run_wix(wxs: &Path, msi: &Path, stage: &Path, arch: &str) -> anyhow::Result<()> {
    let candidates = wix_candidates();

    for candidate in &candidates {
        // 明显不存在的候选（比如机器上没装 WiX 3）直接跳过，不刷屏
        let program = Path::new(&candidate.program);
        if program.components().count() > 1 && !program.exists() {
            continue;
        }

        match invoke_wix(candidate, wxs, msi, stage, arch) {
            Ok(()) => {
                println!("cargo:warning=使用 WiX：{}", candidate.program);
                return Ok(());
            }
            Err(error) => {
                if error.downcast_ref::<NeedsEula>().is_some() {
                    return Err(error);
                }
                eprintln!("（{} 打包失败：{error}）", candidate.program);
            }
        }
    }

    anyhow::bail!(
        "没有可用的 WiX 工具链（试过 {} 个位置：PATH 里的 wix、dotnet 全局工具、\
         Program Files 下的 WiX Toolset、WiX 3 的 candle.exe 等）。\n\
         安装方式（任选其一）：\n\
         · dotnet tool install --global wix      # WiX 4+ CLI，之后 wix 在 PATH 里\n\
         · 从 GitHub Releases 下载 WiX CLI，用环境变量 CODERENDER_WIX 指向 wix.exe 所在目录\n\
         · 安装 WiX 3 后把 candle.exe/light.exe 目录设到环境变量 CODERENDER_WIX\n\
         也可以先 `CODERENDER_MSI_WXS_ONLY=1 cargo build --features msi` 只生成源码检查。",
        candidates.len()
    )
}

/// WiX 5+ 需要接受 OSMF EULA 才能使用。
#[derive(Debug)]
struct NeedsEula;

impl std::fmt::Display for NeedsEula {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "WiX 需要先接受 EULA（OSMF）才能打包。\n\
             · 推荐：运行一次 `wix eula` 永久接受；\n\
             · 或者设置环境变量 CODERENDER_WIX_ACCEPT_EULA=1，构建时会自动加上 --acceptEula。\n\
             详情：https://wixtoolset.org/osmf/"
        )
    }
}

impl std::error::Error for NeedsEula {}

/// 一个 WiX 候选：可执行文件（可能是带空格的完整路径）+ 调用前缀参数。
#[derive(Debug, Clone)]
struct WixCandidate {
    program: String,
    /// `dotnet wix` 这类需要前置参数的写法。
    prefix_args: Vec<String>,
}

impl WixCandidate {
    fn new(program: impl AsRef<Path>) -> Self {
        Self {
            program: program.as_ref().display().to_string(),
            prefix_args: Vec::new(),
        }
    }

    fn with_prefix(program: impl Into<String>, args: &[&str]) -> Self {
        Self {
            program: program.into(),
            prefix_args: args.iter().map(|arg| arg.to_string()).collect(),
        }
    }

    /// 是不是 WiX 3 的 candle/light 工具链。
    fn is_wix3(&self) -> bool {
        let file = Path::new(&self.program)
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        file == "candle.exe" || file == "light.exe"
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        for arg in &self.prefix_args {
            command.arg(arg);
        }
        command
    }
}

fn wix_candidates() -> Vec<WixCandidate> {
    let mut list: Vec<WixCandidate> = Vec::new();

    // 1) 显式指定的位置（可以是 wix.exe、candle.exe 或它们所在的目录）
    if let Some(dir) = std::env::var_os("CODERENDER_WIX") {
        let dir = PathBuf::from(dir);
        if dir.is_file() {
            list.push(WixCandidate::new(dir));
        } else {
            list.push(WixCandidate::new(dir.join("wix.exe")));
            list.push(WixCandidate::new(dir.join("candle.exe")));
        }
    }

    // 2) 常见安装位置（dotnet 全局工具 / winget / chocolatey / scoop / WiX 工具集）
    const RELATIVE_WIX: [&[&str]; 5] = [
        &[".dotnet", "tools", "wix.exe"],
        &["Microsoft", "WinGet", "Links", "wix.exe"],
        &["Microsoft", "WindowsApps", "wix.exe"],
        &["chocolatey", "bin", "wix.exe"],
        &["scoop", "shims", "wix.exe"],
    ];
    for base in [
        std::env::var_os("USERPROFILE").map(PathBuf::from),
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
        std::env::var_os("ProgramData").map(PathBuf::from),
        std::env::var_os("ProgramFiles").map(PathBuf::from),
        std::env::var_os("ProgramFiles(x86)").map(PathBuf::from),
    ]
    .into_iter()
    .flatten()
    {
        for relative in RELATIVE_WIX {
            let mut path = base.clone();
            path.extend(relative);
            list.push(WixCandidate::new(path));
        }
        // WiX 工具集：目录名形如 "WiX Toolset v7.0"（版本号前没有分隔目录）
        for version in ["v7.0", "v6.0", "v5.0", "v4.0"] {
            list.push(WixCandidate::new(
                base.join(format!("WiX Toolset {version}"))
                    .join("bin")
                    .join("wix.exe"),
            ));
        }
        for version in ["v3.14", "v3.11"] {
            list.push(WixCandidate::new(
                base.join(format!("WiX Toolset {version}"))
                    .join("bin")
                    .join("candle.exe"),
            ));
        }
    }

    // 3) PATH 里的 wix / dotnet wix
    list.push(WixCandidate::new(Path::new("wix")));
    list.push(WixCandidate::with_prefix("dotnet", &["wix"]));

    // 去重（保留顺序）
    let mut seen = std::collections::HashSet::new();
    list.retain(|candidate| seen.insert(candidate.program.clone()));
    list
}

/// 执行一次打包：WiX 4+ 用 `wix build`，WiX 3 用 candle + light。
fn invoke_wix(
    candidate: &WixCandidate,
    wxs: &Path,
    msi: &Path,
    stage: &Path,
    arch: &str,
) -> anyhow::Result<()> {
    if candidate.is_wix3() {
        // WiX 3：candle 编译 + light 链接
        let tools = Path::new(&candidate.program)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let obj = wxs.with_extension("wixobj");

        let compiled = candidate
            .command()
            .arg("-arch")
            .arg(arch)
            .arg("-o")
            .arg(&obj)
            .arg(wxs)
            .output()?;
        if !compiled.status.success() {
            anyhow::bail!(
                "candle 失败：{}",
                String::from_utf8_lossy(&compiled.stderr).trim()
            );
        }

        let linked = Command::new(tools.join("light.exe"))
            .arg("-o")
            .arg(msi)
            .arg(&obj)
            .output()?;
        if !linked.status.success() {
            anyhow::bail!(
                "light 失败：{}",
                String::from_utf8_lossy(&linked.stderr).trim()
            );
        }
        return Ok(());
    }

    // WiX 4+：wix build
    let build = |accept_eula: bool| -> anyhow::Result<std::process::Output> {
        let mut command = candidate.command();
        if accept_eula {
            command.arg("--acceptEula");
        }
        Ok(command
            .arg("build")
            .arg("-arch")
            .arg(arch)
            .arg("-bindpath")
            .arg(stage)
            .arg("-o")
            .arg(msi)
            .arg(wxs)
            .output()?)
    };

    let output = build(false)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let combined = format!("{stderr}\n{stdout}");

        // WiX 5+ 的 OSMF EULA：用户明确允许时才自动接受
        if combined.contains("WIX7015") || combined.contains("EULA") {
            if std::env::var_os("CODERENDER_WIX_ACCEPT_EULA").is_some() {
                let retry = build(true)?;
                if retry.status.success() {
                    return Ok(());
                }
            }
            return Err(NeedsEula.into());
        }

        anyhow::bail!("{}\n{}", stderr.trim(), stdout.trim());
    }
    Ok(())
}

/// 从 Cargo.toml 里读 `[package.metadata.msi]` 的简单键值。
fn metadata_value(manifest: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_section = trimmed == "[package.metadata.msi]";
            continue;
        }
        if in_section {
            if let Some((name, value)) = trimmed.split_once('=') {
                if name.trim() == key {
                    return Some(
                        value
                            .trim()
                            .trim_matches('"')
                            .trim_matches('\'')
                            .to_string(),
                    );
                }
            }
        }
    }
    None
}
