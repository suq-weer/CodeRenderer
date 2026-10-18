//! 输入文本的读取与规范化：BOM / CRLF / Tab / NUL / 尾部换行。
//!
//! 规范化只做一次，之后所有阶段（高亮、整形、绘制）都基于同一份文本，
//! 保证 span 的字节偏移与字形一致。

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone)]
pub struct Source {
    pub text: String,
    /// 输入路径（stdin 为 None）
    pub path: Option<PathBuf>,
    /// 用于标题栏 / 默认输出名的显示名
    pub display_name: String,
    /// 是否发生过非 UTF-8 的有损替换
    pub lossy: bool,
}

impl Source {
    pub fn read(input: Option<&Path>, tab_width: usize) -> Result<Self> {
        let (raw, path, display_name) = match input {
            Some(path) if path.as_os_str() != "-" => {
                let bytes = std::fs::read(path)
                    .with_context(|| format!("无法读取输入文件 {}", path.display()))?;
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "untitled".to_string());
                (bytes, Some(path.to_path_buf()), name)
            }
            _ => {
                let mut bytes = Vec::new();
                std::io::stdin()
                    .read_to_end(&mut bytes)
                    .context("无法读取标准输入")?;
                (bytes, None, "untitled".to_string())
            }
        };

        let lossy = std::str::from_utf8(&raw).is_err();
        let decoded = String::from_utf8_lossy(&raw);
        let text = normalize(&decoded, tab_width);

        Ok(Self {
            text,
            path,
            display_name,
            lossy,
        })
    }

    pub fn line_count(&self) -> usize {
        if self.text.is_empty() {
            1
        } else {
            self.text.bytes().filter(|byte| *byte == b'\n').count() + 1
        }
    }

    pub fn default_title(&self) -> String {
        self.display_name.clone()
    }

    pub fn default_output(&self) -> PathBuf {
        let stem = self
            .path
            .as_deref()
            .and_then(|path| path.file_stem())
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "code".to_string());
        PathBuf::from(format!("{stem}.png"))
    }

}

/// 统一的文本规范化：去掉 UTF-8 BOM、CR，展开 Tab，替换 NUL，
/// 并去掉末尾多余的一个换行（避免图片底部多出空行）。
pub fn normalize(input: &str, tab_width: usize) -> String {
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);
    let tab_width = tab_width.max(1);
    let mut out = String::with_capacity(input.len() + input.len() / 8);
    let mut column = 0usize;

    for ch in input.chars() {
        match ch {
            '\r' => {}
            '\n' => {
                out.push('\n');
                column = 0;
            }
            '\t' => {
                let spaces = tab_width - column % tab_width;
                for _ in 0..spaces {
                    out.push(' ');
                }
                column += spaces;
            }
            '\0' => {
                out.push(' ');
                column += 1;
            }
            _ => {
                out.push(ch);
                column += UnicodeWidthChar::width(ch).unwrap_or(0);
            }
        }
    }

    if out.ends_with('\n') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_tabs_by_column() {
        assert_eq!(normalize("\tx", 4), "    x");
        assert_eq!(normalize("ab\tx", 4), "ab  x");
        assert_eq!(normalize("abcd\tx", 4), "abcd    x");
        // 中文占两列（宽字符）
        assert_eq!(normalize("\u{4f60}\tx", 4), "\u{4f60}  x");
    }

    #[test]
    fn strips_bom_crlf_and_trailing_newline() {
        assert_eq!(normalize("\u{feff}a\r\nb\r\n", 4), "a\nb");
        assert_eq!(normalize("a\n\n", 4), "a\n");
    }
}
