//! Embedded assets: binary payloads stored inline in the project file as
//! base64, plus image header sniffing so the core (not the UI) is the
//! authority on an image's pixel size.

use crate::error::{Error, Result};
use crate::model::AssetId;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub id: AssetId,
    pub name: String,
    #[serde(flatten)]
    pub kind: AssetKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AssetKind {
    Image { mime: String, width: u32, height: u32, data: Bytes },
}

/// Immutable shared bytes (cheap to clone into undo history), base64 in JSON.
#[derive(Clone, PartialEq, Eq)]
pub struct Bytes(pub Arc<[u8]>);

impl std::fmt::Debug for Bytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Bytes({} bytes)", self.0.len())
    }
}

impl Serialize for Bytes {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&base64_encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        base64_decode(&s).map(|v| Bytes(v.into())).map_err(serde::de::Error::custom)
    }
}

/// Pixel size and MIME type read from an image file's header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageInfo {
    pub mime: &'static str,
    pub width: u32,
    pub height: u32,
}

/// Recognizes PNG, JPEG and GIF from their headers.
pub fn sniff_image(data: &[u8]) -> Result<ImageInfo> {
    let bad = || Error::Invalid("unsupported or corrupt image (expected PNG, JPEG or GIF)".into());
    let be32 = |i: usize| u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
    if data.len() >= 24 && data.starts_with(b"\x89PNG\r\n\x1a\n") && &data[12..16] == b"IHDR" {
        return Ok(ImageInfo { mime: "image/png", width: be32(16), height: be32(20) }).and_then(nonzero);
    }
    if data.len() >= 10 && (data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a")) {
        let le16 = |i: usize| u16::from_le_bytes([data[i], data[i + 1]]) as u32;
        return nonzero(ImageInfo { mime: "image/gif", width: le16(6), height: le16(8) });
    }
    if data.len() >= 4 && data[0] == 0xFF && data[1] == 0xD8 {
        // Walk JPEG segments to the first start-of-frame marker.
        let mut i = 2;
        while i + 9 < data.len() {
            if data[i] != 0xFF {
                return Err(bad());
            }
            let marker = data[i + 1];
            if marker == 0xFF {
                i += 1;
                continue;
            }
            let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
            let is_sof = matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
            if is_sof {
                let h = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
                let w = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
                return nonzero(ImageInfo { mime: "image/jpeg", width: w, height: h });
            }
            i += 2 + len;
        }
    }
    Err(bad())
}

fn nonzero(info: ImageInfo) -> Result<ImageInfo> {
    if info.width == 0 || info.height == 0 {
        Err(Error::Invalid("image has zero size".into()))
    } else {
        Ok(info)
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for k in 0..4 {
            if k <= chunk.len() {
                out.push(B64[(n >> (18 - 6 * k) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn base64_decode(s: &str) -> std::result::Result<Vec<u8>, String> {
    let val = |c: u8| -> std::result::Result<u32, String> {
        match c {
            b'A'..=b'Z' => Ok((c - b'A') as u32),
            b'a'..=b'z' => Ok((c - b'a' + 26) as u32),
            b'0'..=b'9' => Ok((c - b'0' + 52) as u32),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => Err(format!("invalid base64 character {:?}", c as char)),
        }
    };
    let bytes: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
    if !bytes.len().is_multiple_of(4) {
        return Err("base64 length is not a multiple of 4".into());
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for (ci, chunk) in bytes.chunks(4).enumerate() {
        let last = ci == bytes.len() / 4 - 1;
        let pad = chunk.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return Err("misplaced base64 padding".into());
        }
        let mut n = 0u32;
        for &c in &chunk[..4 - pad] {
            n = n << 6 | val(c)?;
        }
        n <<= 6 * pad as u32;
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_and_matches_rfc4648() {
        for (raw, enc) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foobar", "Zm9vYmFy")] {
            assert_eq!(base64_encode(raw.as_bytes()), enc);
            assert_eq!(base64_decode(enc).unwrap(), raw.as_bytes());
        }
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(base64_decode(&base64_encode(&all)).unwrap(), all);
        assert!(base64_decode("Zg=").is_err());
        assert!(base64_decode("Z===").is_err());
        assert!(base64_decode("Zg==Zg==").is_err());
        assert!(base64_decode("Z!==").is_err());
    }

    #[test]
    fn sniffs_png_gif_jpeg_headers() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend_from_slice(&640u32.to_be_bytes());
        png.extend_from_slice(&480u32.to_be_bytes());
        assert_eq!(sniff_image(&png).unwrap(), ImageInfo { mime: "image/png", width: 640, height: 480 });

        let gif = b"GIF89a\x20\x03\x58\x02rest";
        assert_eq!(sniff_image(gif).unwrap(), ImageInfo { mime: "image/gif", width: 800, height: 600 });

        // SOI, APP0 (len 16), SOF0 with h=0x0100, w=0x0200.
        let mut jpg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        jpg.extend_from_slice(&[0; 14]);
        jpg.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08, 0x01, 0x00, 0x02, 0x00, 0x03]);
        assert_eq!(sniff_image(&jpg).unwrap(), ImageInfo { mime: "image/jpeg", width: 512, height: 256 });

        assert!(sniff_image(b"hello world, not an image").is_err());
    }
}
