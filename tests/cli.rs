//! 命令行端到端测试。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_coderender")
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "coderender-test-{}-{tag}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn themes_lists_all_presets() {
    let output = Command::new(bin()).arg("themes").output().unwrap();
    assert!(output.status.success(), "{:?}", output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    for id in ["latte", "frappe", "macchiato", "mocha"] {
        assert!(stdout.contains(id), "缺少配色 {id}: {stdout}");
    }
    for id in ["macos", "windows"] {
        assert!(stdout.contains(id), "缺少窗口装饰 {id}: {stdout}");
    }
}

#[test]
fn renders_png_from_file() {
    let dir = temp_dir("file");
    let input = dir.join("demo.rs");
    let output = dir.join("demo.png");
    fs::write(&input, "fn main() {\n    println!(\"hi\");\n}\n").unwrap();

    let status = Command::new(bin())
        .args([
            "render",
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--scale",
            "2",
            "--theme",
            "latte",
            "--window",
            "windows",
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let image = image::open(&output).unwrap();
    assert!(image.width() > 400);
    assert!(image.height() > 200);
    // 2x 输出的宽高都是偶数且明显大于 1x 的典型尺寸
    assert_eq!(image.width() % 2, 0);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn reads_code_from_stdin() {
    use std::io::Write;
    use std::process::Stdio;

    let dir = temp_dir("stdin");
    let output = dir.join("stdin.png");

    let mut child = Command::new(bin())
        .args([
            "render",
            "--lang",
            "python",
            "-o",
            output.to_str().unwrap(),
        ])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"def hello():\n    print('hi')\n")
        .unwrap();
    assert!(child.wait().unwrap().success());
    assert!(output.exists());

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn rejects_out_of_range_scale() {
    let output = Command::new(bin())
        .args(["render", "whatever.rs", "--scale", "9"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("倍率"), "stderr: {stderr}");
}

#[test]
fn exports_builtin_pack() {
    let dir = temp_dir("export");
    let pack = dir.join("pack.toml");
    let status = Command::new(bin())
        .args(["themes", "--export", pack.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success());
    let text = fs::read_to_string(&pack).unwrap();
    assert!(text.contains("[palettes.mocha]"));
    assert!(text.contains("[decorations.macos.controls]"));

    // 导出的包应当能再次被加载并渲染
    let output = dir.join("pack.png");
    let status = Command::new(bin())
        .args([
            "render",
            "samples/sample.rs",
            "--theme-pack",
            pack.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(output.exists());

    fs::remove_dir_all(&dir).ok();
}

/// 语法扩展包全流程：模板 → 安装 → 渲染新语言 → 打包 → 卸载 → 从分发包安装 → 再渲染
#[test]
fn syntax_pack_full_lifecycle() {
    let dir = temp_dir("syntaxpack");
    let home = dir.join("home");
    let code = dir.join("demo.mylang");
    fs::write(&code, "# 注释\nfn main() {\n    let x = 42\n}\n").unwrap();

    let render = |output: &Path| {
        Command::new(bin())
            .env("CODERENDER_HOME", &home)
            .args([
                "render",
                code.to_str().unwrap(),
                "-o",
                output.to_str().unwrap(),
            ])
            .status()
            .unwrap()
    };

    // 1. 生成模板
    let dev = dir.join("mylang-extra");
    let status = Command::new(bin())
        .args(["pack", "new", "MyLang Extra", "--kind", "syntax"])
        .current_dir(&dir)
        .status()
        .unwrap();
    assert!(status.success());
    assert!(dev.join("pack.toml").is_file());
    assert!(dev.join("syntaxes").join("MyLang.sublime-syntax").is_file());

    // 2. 安装开发目录
    let status = Command::new(bin())
        .env("CODERENDER_HOME", &home)
        .args(["pack", "install", dev.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success());

    // 3. 用扩展包渲染 .mylang
    let output = dir.join("from-folder.png");
    assert!(render(&output).success());
    assert!(output.exists());

    // 4. 打包成 .crpack，卸载后从分发包重新安装
    let distribution = dir.join("mylang-extra.crpack");
    let status = Command::new(bin())
        .args([
            "pack",
            "build",
            dev.to_str().unwrap(),
            "-o",
            distribution.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(distribution.exists());

    let status = Command::new(bin())
        .env("CODERENDER_HOME", &home)
        .args(["pack", "uninstall", "mylang-extra"])
        .status()
        .unwrap();
    assert!(status.success());

    let status = Command::new(bin())
        .env("CODERENDER_HOME", &home)
        .args(["pack", "install", distribution.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success());

    let output = dir.join("from-distribution.png");
    assert!(render(&output).success());

    // 5. pack list 能看到它，--verbose 列出语法
    let output = Command::new(bin())
        .env("CODERENDER_HOME", &home)
        .args(["pack", "list", "--kind", "syntax", "--verbose"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("mylang-extra"), "{stdout}");
    assert!(stdout.contains("MyLang"), "{stdout}");

    fs::remove_dir_all(&dir).ok();
}

/// 主题包安装后可以直接用 `--theme` 选中
#[test]
fn installed_theme_pack_is_usable() {
    let dir = temp_dir("themepack");
    let home = dir.join("home");
    let dev = dir.join("sunset");

    std::fs::create_dir_all(&dev).unwrap();
    std::fs::write(
        dev.join("pack.toml"),
        r##"
[pack]
id = "sunset"
kind = "theme"
name = "Sunset"
version = "1.0.0"

[palettes.dusk]
background = "#2d1b3e"
foreground = "#ffe0b2"
gutter = "#8d6e9e"

[palettes.dusk.syntax]
keyword = "#ff8a65"
string = "#a5d6a7"

[decorations.minimal]
title_height = 30
corner_radius = 6
padding = 20

[decorations.minimal.controls]
style = "none"
"##,
    )
    .unwrap();

    let status = Command::new(bin())
        .args(["pack", "install", dev.to_str().unwrap()])
        .env("CODERENDER_HOME", &home)
        .status()
        .unwrap();
    assert!(status.success());

    let output = dir.join("sunset.png");
    let status = Command::new(bin())
        .env("CODERENDER_HOME", &home)
        .args([
            "render",
            "samples/sample.rs",
            "--theme",
            "dusk",
            "--window",
            "minimal",
            "-o",
            output.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(output.exists());

    // 拼错主题名时给出「您是否指的是……」
    let output = Command::new(bin())
        .env("CODERENDER_HOME", &home)
        .args([
            "render",
            "samples/sample.rs",
            "--theme",
            "duskk",
            "-o",
            "x.png",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("您是否指的是"), "{stderr}");
    assert!(stderr.contains("dusk"), "{stderr}");

    fs::remove_dir_all(&dir).ok();
}

/// 内置语法集可以导出为扩展包再安装
#[test]
fn builtin_syntax_pack_can_be_exported_and_installed() {
    let dir = temp_dir("builtinexport");
    let home = dir.join("home");
    let exported = dir.join("builtin-syntaxes");

    let status = Command::new(bin())
        .args([
            "pack",
            "builtin-export",
            "-o",
            exported.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(exported.join("pack.toml").is_file());
    assert!(exported
        .join("syntaxes")
        .join("syntaxes.packdump")
        .is_file());

    let status = Command::new(bin())
        .args(["pack", "install", exported.to_str().unwrap()])
        .env("CODERENDER_HOME", &home)
        .status()
        .unwrap();
    assert!(status.success());

    // 安装后 Rust 仍然正常高亮（内置规则与安装的包内容一致）
    let output = dir.join("still-rust.png");
    let status = Command::new(bin())
        .env("CODERENDER_HOME", &home)
        .args([
            "render",
            "samples/sample.rs",
            "-o",
            output.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn typo_in_lang_fails_with_suggestion() {
    let output = Command::new(bin())
        .args(["render", "samples/sample.rs", "--lang", "rusttt", "-o", "x.png"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("您是否指的是"), "{stderr}");
    assert!(stderr.contains("Rust"), "{stderr}");
}
