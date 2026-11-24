//! Minimal PNG encoder (stored deflate, no compression) for icon generation
//! and UI previews. Not used at runtime by the overlay itself.

#![allow(dead_code)]

use crate::ui::Canvas;

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

/// RGBA PNG. With `backdrop = Some(grey)`, translucency is flattened onto that
/// grey (handy for eyeballing); with `None` the real alpha is kept.
pub fn encode(c: &Canvas, backdrop: Option<u8>) -> Vec<u8> {
    let mut raw = Vec::with_capacity((c.w * c.h * 4 + c.h) as usize);
    for y in 0..c.h as usize {
        raw.push(0u8);
        for p in c.px[y * c.w as usize * 4..(y + 1) * c.w as usize * 4].chunks(4) {
            match backdrop {
                Some(g) => {
                    let a = p[3] as u32;
                    for &ch in &p[..3] {
                        raw.push(((ch as u32 * a + g as u32 * (255 - a)) / 255) as u8);
                    }
                    raw.push(255);
                }
                None => raw.extend_from_slice(p),
            }
        }
    }
    let mut z = vec![0x78, 0x01];
    let n = raw.chunks(65535).count();
    for (i, chunk) in raw.chunks(65535).enumerate() {
        z.push((i + 1 == n) as u8);
        z.extend_from_slice(&(chunk.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(chunk.len() as u16)).to_le_bytes());
        z.extend_from_slice(chunk);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut chunk = |ty: &[u8], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut td = ty.to_vec();
        td.extend_from_slice(data);
        out.extend_from_slice(&td);
        out.extend_from_slice(&crc32(&td).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&c.w.to_be_bytes());
    ihdr.extend_from_slice(&c.h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    out
}

/// Windows .ico holding one PNG image per size (supported since Vista).
pub fn ico(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, data) in images {
        let d = if *size >= 256 { 0 } else { *size as u8 };
        out.extend_from_slice(&[d, d, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in images {
        out.extend_from_slice(data);
    }
    out
}
