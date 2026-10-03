use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Straight (non-premultiplied) 8-bit RGBA color.
/// Serialized as `"#rrggbb"` when opaque, `"#rrggbbaa"` otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const BLACK: Color = Color::rgb(0, 0, 0);
    pub const WHITE: Color = Color::rgb(255, 255, 255);

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Color { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Color { r, g, b, a }
    }

    pub fn to_hex(&self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }

    /// Accepts `#rgb`, `#rrggbb` and `#rrggbbaa` (leading `#` optional).
    pub fn parse_hex(s: &str) -> Option<Color> {
        let h = s.strip_prefix('#').unwrap_or(s);
        if !h.is_ascii() {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
        match h.len() {
            3 => {
                let n = |i: usize| u8::from_str_radix(&h[i..i + 1], 16).ok().map(|v| v * 17);
                Some(Color::rgb(n(0)?, n(1)?, n(2)?))
            }
            6 => Some(Color::rgb(byte(0)?, byte(2)?, byte(4)?)),
            8 => Some(Color::rgba(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
            _ => None,
        }
    }

    /// CSS color string for Canvas2D-style backends.
    pub fn to_css(&self) -> String {
        if self.a == 255 {
            self.to_hex()
        } else {
            format!("rgba({},{},{},{})", self.r, self.g, self.b, self.a as f64 / 255.0)
        }
    }
}

/// Flash-style color transform: `out = clamp(in * mul + add)` per channel
/// (channels in 0..=255). Applied per drawn primitive and composed down the
/// instance tree (child first, then parent).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorTransform {
    /// r, g, b, a multipliers.
    pub mul: [f64; 4],
    /// r, g, b, a offsets in 0..=255 units.
    pub add: [f64; 4],
}

impl Default for ColorTransform {
    fn default() -> Self {
        ColorTransform::IDENTITY
    }
}

impl ColorTransform {
    pub const IDENTITY: ColorTransform = ColorTransform { mul: [1.0; 4], add: [0.0; 4] };

    pub fn alpha(alpha: f64) -> Self {
        ColorTransform { mul: [1.0, 1.0, 1.0, alpha], add: [0.0; 4] }
    }

    /// Mixes rgb toward `color` by `amount` (0..=1); alpha is untouched.
    pub fn tint(color: Color, amount: f64) -> Self {
        let k = 1.0 - amount;
        ColorTransform {
            mul: [k, k, k, 1.0],
            add: [color.r as f64 * amount, color.g as f64 * amount, color.b as f64 * amount, 0.0],
        }
    }

    /// `self` applied first, then `outer`.
    pub fn then(&self, outer: &ColorTransform) -> ColorTransform {
        let mut r = ColorTransform::IDENTITY;
        for i in 0..4 {
            r.mul[i] = self.mul[i] * outer.mul[i];
            r.add[i] = self.add[i] * outer.mul[i] + outer.add[i];
        }
        r
    }

    pub fn apply(&self, c: Color) -> Color {
        let ch = |v: u8, i: usize| (v as f64 * self.mul[i] + self.add[i]).round().clamp(0.0, 255.0) as u8;
        Color::rgba(ch(c.r, 0), ch(c.g, 1), ch(c.b, 2), ch(c.a, 3))
    }

    pub fn is_identity(&self) -> bool {
        *self == ColorTransform::IDENTITY
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Color::parse_hex(&s).ok_or_else(|| serde::de::Error::custom(format!("invalid color {s:?}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        for c in [Color::rgb(1, 2, 3), Color::rgba(255, 128, 0, 17), Color::WHITE] {
            assert_eq!(Color::parse_hex(&c.to_hex()), Some(c));
        }
        assert_eq!(Color::parse_hex("#fa0"), Some(Color::rgb(255, 170, 0)));
        assert_eq!(Color::parse_hex("#12345"), None);
        assert_eq!(Color::parse_hex("#gg0000"), None);
        assert_eq!(Color::parse_hex("#é0000"), None);
    }

    #[test]
    fn color_transforms_compose_child_first() {
        let tint = ColorTransform::tint(Color::rgb(255, 0, 0), 0.5);
        let half = ColorTransform::alpha(0.5);
        let c = tint.then(&half).apply(Color::rgb(0, 0, 200));
        assert_eq!(c, Color::rgba(128, 0, 100, 128));
        assert_eq!(ColorTransform::IDENTITY.then(&tint), tint);
        assert_eq!(ColorTransform::tint(Color::WHITE, 1.0).apply(Color::BLACK), Color::WHITE);
    }
}
