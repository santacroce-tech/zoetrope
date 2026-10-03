//! Static text: shaping (rustybuzz: kerning, ligatures, complex scripts),
//! line breaking, alignment, and glyph outlines → vector paths.
//!
//! Text is laid out by the core from embedded font assets, so a text
//! element renders identically in the editor and in exports, on machines
//! that don't have the font installed. Renderers only ever see paths.
//!
//! Content coordinates: the text box's top-left is the origin; the box is
//! `width × height` (see `TextLayout`).

use crate::geom::{Path, PathCmd};
use crate::math::Point;
use crate::model::AssetId;
use crate::paint::Paint;
use rustybuzz::ttf_parser;
use serde::{Deserialize, Serialize};

/// The bundled default font ("Zoetrope Sans", a Latin subset of Lato; OFL).
pub const DEFAULT_FONT: &[u8] = include_bytes!("../assets/fonts/ZoetropeSans-Regular.ttf");
pub const DEFAULT_FONT_NAME: &str = "Zoetrope Sans";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

fn default_line_height() -> f64 {
    1.25
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextBlock {
    /// The text; `\n` starts a new line.
    pub text: String,
    /// A font asset.
    pub font: AssetId,
    /// Font size (em) in content units.
    pub size: f64,
    pub fill: Paint,
    #[serde(default)]
    pub align: TextAlign,
    /// Extra space after every glyph, in content units.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub letter_spacing: f64,
    /// Baseline-to-baseline distance as a multiple of `size`.
    #[serde(default = "default_line_height")]
    pub line_height: f64,
    /// Wrap width. `None`: one line per `\n`, box as wide as the longest line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
}

impl TextBlock {
    pub fn validate(&self) -> crate::Result<()> {
        let bad = |m: &str| Err(crate::Error::Invalid(format!("text: {m}")));
        if !(self.size.is_finite() && self.size > 0.0) {
            return bad("size must be positive");
        }
        if !(self.line_height.is_finite() && self.line_height > 0.0) {
            return bad("line height must be positive");
        }
        if !self.letter_spacing.is_finite() {
            return bad("letter spacing must be finite");
        }
        if self.width.is_some_and(|w| !(w.is_finite() && w > 0.0)) {
            return bad("box width must be positive");
        }
        self.fill.validate()
    }
}

/// Laid-out text in content coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct TextLayout {
    /// All glyph outlines, ready to fill.
    pub path: Path,
    /// Box size (the box always spans at least one line, even when empty).
    pub width: f64,
    pub height: f64,
    pub lines: usize,
}

/// The family name of a font file, or `None` if it isn't a usable font.
pub fn font_family(data: &[u8]) -> Option<String> {
    let face = ttf_parser::Face::parse(data, 0).ok()?;
    let name = |id: u16| {
        face.names().into_iter().find(|n| n.name_id == id && n.is_unicode()).and_then(|n| n.to_string())
    };
    name(ttf_parser::name_id::TYPOGRAPHIC_FAMILY).or_else(|| name(ttf_parser::name_id::FAMILY)).or(Some("Font".into()))
}

struct Glyph {
    id: u16,
    x: f64,
    y: f64,
    advance: f64,
    /// Byte offset of its cluster in the paragraph.
    cluster: usize,
}

/// Lays out `block` with the font `font_data`. `None` if the font can't be parsed.
pub fn layout(block: &TextBlock, font_data: &[u8]) -> Option<TextLayout> {
    let face = rustybuzz::Face::from_slice(font_data, 0)?;
    let k = block.size / face.units_per_em() as f64;
    let ascender = face.ascender() as f64 * k;
    let line_advance = block.size * block.line_height;

    // Shape each paragraph, then break into lines.
    let mut lines: Vec<Vec<Glyph>> = Vec::new();
    for paragraph in block.text.split('\n') {
        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str(paragraph);
        let shaped = rustybuzz::shape(&face, &[], buffer);
        let glyphs: Vec<Glyph> = shaped
            .glyph_infos()
            .iter()
            .zip(shaped.glyph_positions())
            .map(|(info, pos)| Glyph {
                id: info.glyph_id as u16,
                x: pos.x_offset as f64 * k,
                y: pos.y_offset as f64 * k,
                advance: pos.x_advance as f64 * k + block.letter_spacing,
                cluster: info.cluster as usize,
            })
            .collect();
        let is_space = |g: &Glyph| paragraph[g.cluster..].chars().next().is_some_and(char::is_whitespace);
        match block.width {
            None => lines.push(glyphs),
            Some(max) => {
                // Greedy wrap at whitespace; a word longer than the box overflows.
                let mut line: Vec<Glyph> = Vec::new();
                let mut x = 0.0;
                let mut last_space: Option<usize> = None;
                for g in glyphs {
                    if !line.is_empty() && x + g.advance > max && !is_space(&g) {
                        if let Some(sp) = last_space {
                            let rest = line.split_off(sp + 1);
                            line.pop(); // the breaking space
                            lines.push(std::mem::replace(&mut line, rest));
                            x = line.iter().map(|g| g.advance).sum();
                            last_space = None;
                        }
                    }
                    if is_space(&g) {
                        last_space = Some(line.len());
                    }
                    x += g.advance;
                    line.push(g);
                }
                lines.push(line);
            }
        }
    }

    let widths: Vec<f64> = lines
        .iter()
        .map(|l| {
            // Trailing letter spacing doesn't count toward the line's width.
            let w: f64 = l.iter().map(|g| g.advance).sum();
            if l.is_empty() { 0.0 } else { w - block.letter_spacing }
        })
        .collect();
    let box_width = block.width.unwrap_or_else(|| widths.iter().cloned().fold(0.0, f64::max));
    let mut path = Path::default();
    for (i, (line, w)) in lines.iter().zip(&widths).enumerate() {
        let baseline = ascender + i as f64 * line_advance;
        let mut x = match block.align {
            TextAlign::Left => 0.0,
            TextAlign::Center => (box_width - w) / 2.0,
            TextAlign::Right => box_width - w,
        };
        for g in line {
            let mut b = Builder { path: &mut path, ox: x + g.x, oy: baseline - g.y, k };
            face.outline_glyph(ttf_parser::GlyphId(g.id), &mut b);
            x += g.advance;
        }
    }
    let n = lines.len().max(1);
    let height = ascender - (face.descender() as f64 * k) + (n - 1) as f64 * line_advance;
    Some(TextLayout { path, width: box_width.max(0.0), height, lines: n })
}

struct Builder<'a> {
    path: &'a mut Path,
    ox: f64,
    oy: f64,
    k: f64,
}

impl Builder<'_> {
    fn p(&self, x: f32, y: f32) -> Point {
        Point::new(self.ox + x as f64 * self.k, self.oy - y as f64 * self.k)
    }
}

impl ttf_parser::OutlineBuilder for Builder<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let p = self.p(x, y);
        self.path.cmds.push(PathCmd::MoveTo(p));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.p(x, y);
        self.path.cmds.push(PathCmd::LineTo(p));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (c, p) = (self.p(x1, y1), self.p(x, y));
        self.path.cmds.push(PathCmd::QuadTo(c, p));
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (c1, c2, p) = (self.p(x1, y1), self.p(x2, y2), self.p(x, y));
        self.path.cmds.push(PathCmd::CubicTo(c1, c2, p));
    }
    fn close(&mut self) {
        self.path.cmds.push(PathCmd::Close);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;

    fn block(text: &str) -> TextBlock {
        TextBlock {
            text: text.into(),
            font: AssetId(1),
            size: 40.0,
            fill: Paint::solid(Color::BLACK),
            align: TextAlign::Left,
            letter_spacing: 0.0,
            line_height: 1.25,
            width: None,
        }
    }

    #[test]
    fn default_font_parses_and_names_itself() {
        assert_eq!(font_family(DEFAULT_FONT).as_deref(), Some("Zoetrope Sans"));
        assert!(font_family(b"not a font").is_none());
    }

    #[test]
    fn lays_out_lines_and_glyphs() {
        let l = layout(&block("Hello\nWorld"), DEFAULT_FONT).unwrap();
        assert_eq!(l.lines, 2);
        assert!(l.width > 80.0 && l.width < 140.0, "{}", l.width);
        assert!(l.height > 90.0 && l.height < 110.0, "{}", l.height);
        let b = l.path.bounds(&crate::Matrix::IDENTITY).unwrap();
        assert!(b.min.x >= -1.0 && b.max.x <= l.width + 1.0);
        assert!(b.min.y >= 0.0 && b.max.y <= l.height);
        // Empty text still has a one-line box.
        let e = layout(&block(""), DEFAULT_FONT).unwrap();
        assert_eq!(e.lines, 1);
        assert!(e.height > 40.0 && e.path.cmds.is_empty());
    }

    #[test]
    fn kerning_is_applied() {
        // "AV" kerns tighter than A + V measured separately.
        let pair = layout(&block("AV"), DEFAULT_FONT).unwrap().width;
        let sep = layout(&block("A"), DEFAULT_FONT).unwrap().width + layout(&block("V"), DEFAULT_FONT).unwrap().width;
        assert!(pair < sep - 0.5, "AV {pair} vs {sep}");
    }

    #[test]
    fn wraps_aligns_and_spaces() {
        let mut b = block("one two three four");
        b.width = Some(120.0);
        let l = layout(&b, DEFAULT_FONT).unwrap();
        assert!(l.lines >= 2);
        assert_eq!(l.width, 120.0);
        let mut c = b.clone();
        c.align = TextAlign::Right;
        let right = layout(&c, DEFAULT_FONT).unwrap().path.bounds(&crate::Matrix::IDENTITY).unwrap();
        assert!(right.max.x > 118.0 && right.max.x <= 121.0);
        let mut s = block("abc");
        let w0 = layout(&s, DEFAULT_FONT).unwrap().width;
        s.letter_spacing = 5.0;
        assert!((layout(&s, DEFAULT_FONT).unwrap().width - (w0 + 10.0)).abs() < 1e-9);
    }
}
