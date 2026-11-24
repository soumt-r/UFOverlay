//! Unicode text (Korean window titles, key labels) via plain GDI.
//! One DC + DIB per redraw, however many labels; only used when a static image
//! is redrawn, never per frame.

use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::Gdi::*;

use crate::ui::{Canvas, Rgba};

#[derive(Clone, Copy)]
pub enum Align {
    Left,
    Center,
}

pub struct Label {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub text: String,
    pub px: i32,
    pub color: Rgba,
    pub align: Align,
    pub bold: bool,
    /// Font face; None = the current UI language's font.
    pub face: Option<&'static str>,
}

#[allow(clippy::too_many_arguments)] // a positioned label is just that many numbers
pub fn label(x: i32, y: i32, w: i32, h: i32, text: impl Into<String>, px: i32, color: Rgba, align: Align, bold: bool) -> Label {
    Label { x, y, w, h, text: text.into(), px, color, align, bold, face: None }
}

/// Draw all labels onto the canvas, keeping the canvas alpha.
pub fn draw_all(c: &mut Canvas, labels: &[Label]) {
    if labels.is_empty() {
        return;
    }
    let (w, h) = (c.w as i32, c.h as i32);
    unsafe {
        let dc = CreateCompatibleDC(None);
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let Ok(bmp) = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            let _ = DeleteDC(dc);
            return;
        };
        let old_bmp = SelectObject(dc, bmp.into());
        let dib = std::slice::from_raw_parts_mut(bits as *mut u8, (w * h * 4) as usize);
        for (d, s) in dib.chunks_exact_mut(4).zip(c.px.chunks_exact(4)) {
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
        }
        SetBkMode(dc, TRANSPARENT);
        let mut fonts: Vec<((i32, bool, &'static str), HFONT)> = Vec::new();
        let first_font = SelectObject(dc, GetStockObject(DEFAULT_GUI_FONT));
        for l in labels {
            if l.text.is_empty() {
                continue;
            }
            let face = l.face.unwrap_or_else(crate::i18n::font);
            let key = (l.px, l.bold, face);
            let font = match fonts.iter().find(|f| f.0 == key) {
                Some(f) => f.1,
                None => {
                    let wface: Vec<u16> = face.encode_utf16().chain(std::iter::once(0)).collect();
                    let f = CreateFontW(
                        -l.px,
                        0,
                        0,
                        0,
                        if l.bold { FW_BOLD.0 as i32 } else { FW_NORMAL.0 as i32 },
                        0,
                        0,
                        0,
                        DEFAULT_CHARSET,
                        OUT_DEFAULT_PRECIS,
                        CLIP_DEFAULT_PRECIS,
                        ANTIALIASED_QUALITY,
                        (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
                        windows::core::PCWSTR(wface.as_ptr()),
                    );
                    fonts.push((key, f));
                    f
                }
            };
            SelectObject(dc, font.into());
            SetTextColor(dc, COLORREF(l.color[0] as u32 | (l.color[1] as u32) << 8 | (l.color[2] as u32) << 16));
            let mut wide: Vec<u16> = l.text.encode_utf16().collect();
            let mut r = RECT { left: l.x, top: l.y, right: l.x + l.w, bottom: l.y + l.h };
            let fmt = DT_SINGLELINE
                | DT_VCENTER
                | DT_END_ELLIPSIS
                | DT_NOPREFIX
                | match l.align {
                    Align::Left => DT_LEFT,
                    Align::Center => DT_CENTER,
                };
            DrawTextW(dc, &mut wide, &mut r, fmt);
        }
        let _ = GdiFlush();
        for (s, d) in dib.chunks_exact(4).zip(c.px.chunks_exact_mut(4)) {
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
        }
        SelectObject(dc, first_font);
        SelectObject(dc, old_bmp);
        for (_, f) in fonts {
            let _ = DeleteObject(f.into());
        }
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(dc);
    }
}
