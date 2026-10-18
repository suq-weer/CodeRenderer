//! 颜色值：解析、混合，以及与 cosmic-text / tiny-skia 之间的转换。

use anyhow::{bail, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const TRANSPARENT: Self = Self::new(0, 0, 0, 0);

    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub const fn opaque(r: u8, g: u8, b: u8) -> Self {
        Self::new(r, g, b, 255)
    }

    /// 解析 `#rgb` / `#rgba` / `#rrggbb` / `#rrggbbaa`（`#` 可省略），
    /// 另外接受 `transparent`。
    pub fn from_hex(input: &str) -> Result<Self> {
        let s = input.trim();
        if s.eq_ignore_ascii_case("transparent") || s.eq_ignore_ascii_case("none") {
            return Ok(Self::TRANSPARENT);
        }
        let s = s
            .strip_prefix('#')
            .or_else(|| s.strip_prefix("0x"))
            .or_else(|| s.strip_prefix("0X"))
            .unwrap_or(s);

        let nibble = |c: u8| -> Result<u8> {
            match c {
                b'0'..=b'9' => Ok(c - b'0'),
                b'a'..=b'f' => Ok(c - b'a' + 10),
                b'A'..=b'F' => Ok(c - b'A' + 10),
                _ => bail!("颜色 `{input}` 含有非法十六进制字符"),
            }
        };
        let bytes = s.as_bytes();
        let byte_at = |i: usize| -> Result<u8> { Ok(nibble(bytes[i])? * 16 + nibble(bytes[i + 1])?) };

        match bytes.len() {
            3 | 4 => {
                let expand = |i: usize| -> Result<u8> { Ok(nibble(bytes[i])? * 17) };
                Ok(Self::new(
                    expand(0)?,
                    expand(1)?,
                    expand(2)?,
                    if bytes.len() == 4 { expand(3)? } else { 255 },
                ))
            }
            6 | 8 => Ok(Self::new(
                byte_at(0)?,
                byte_at(2)?,
                byte_at(4)?,
                if bytes.len() == 8 { byte_at(6)? } else { 255 },
            )),
            _ => bail!("颜色 `{input}` 格式不正确，应为 #rgb / #rgba / #rrggbb / #rrggbbaa"),
        }
    }

    pub fn to_cosmic(self) -> cosmic_text::Color {
        cosmic_text::Color::rgba(self.r, self.g, self.b, self.a)
    }

    pub fn to_skia(self) -> tiny_skia::Color {
        tiny_skia::Color::from_rgba8(self.r, self.g, self.b, self.a)
    }

    /// 线性（sRGB 空间）插值，`t = 0` 返回 self，`t = 1` 返回 other。
    pub fn mix(self, other: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let lerp = |a: u8, b: u8| -> u8 {
            (a as f32 + (b as f32 - a as f32) * t).round().clamp(0.0, 255.0) as u8
        };
        Self {
            r: lerp(self.r, other.r),
            g: lerp(self.g, other.g),
            b: lerp(self.b, other.b),
            a: lerp(self.a, other.a),
        }
    }

    pub fn with_alpha(self, a: f32) -> Self {
        Self {
            a: (a.clamp(0.0, 1.0) * 255.0).round() as u8,
            ..self
        }
    }

}

impl From<syntect::highlighting::Color> for Rgba {
    fn from(c: syntect::highlighting::Color) -> Self {
        Self::new(c.r, c.g, c.b, c.a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_variants() {
        assert_eq!(Rgba::from_hex("#ff0000").unwrap(), Rgba::opaque(255, 0, 0));
        assert_eq!(Rgba::from_hex("f00").unwrap(), Rgba::opaque(255, 0, 0));
        assert_eq!(Rgba::from_hex("#1e1e2e").unwrap(), Rgba::opaque(30, 30, 46));
        assert_eq!(Rgba::from_hex("#1e1e2e80").unwrap(), Rgba::new(30, 30, 46, 128));
        assert_eq!(Rgba::from_hex("transparent").unwrap(), Rgba::TRANSPARENT);
        assert!(Rgba::from_hex("#12345").is_err());
        assert!(Rgba::from_hex("#gggggg").is_err());
    }

    #[test]
    fn mixes() {
        let m = Rgba::opaque(0, 0, 0).mix(Rgba::opaque(255, 255, 255), 0.5);
        assert_eq!(m, Rgba::opaque(128, 128, 128));
    }
}
