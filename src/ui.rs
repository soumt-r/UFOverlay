//! Tiny CPU painter for the few images UFOverlay draws: toolbar pill, menu,
//! wrist widget, keyboard, cursor and the UFO mascot. Shapes are signed-distance
//! fields with 1-px anti-aliasing; text goes through GDI (text.rs).
//! Everything here is redrawn only on change — except the wrist mascot, which
//! animates at a few fps *only while you are looking at your wrist*.

use crate::i18n::tr;
use crate::ime::ImeState;
use crate::text::{self, label, Align, Label};

pub type Rgba = [u8; 4];

// UFOverlay palette: night sky, mint tractor beam, lavender saucer, warm lights.
pub const SKY: Rgba = [21, 24, 38, 240];
pub const SKY_2: Rgba = [33, 37, 58, 255];
pub const SKY_3: Rgba = [46, 51, 78, 255];
pub const MINT: Rgba = [124, 242, 201, 255];
pub const LAVENDER: Rgba = [185, 167, 255, 255];
pub const BULB: Rgba = [255, 214, 107, 255];
pub const BLUSH: Rgba = [255, 140, 170, 150];
pub const TEXT: Rgba = [240, 242, 250, 255];
pub const DIM: Rgba = [150, 156, 184, 255];
pub const WARN: Rgba = [255, 170, 90, 255];
pub const CLOSE: Rgba = [232, 98, 120, 255];
const INK: Rgba = [30, 28, 52, 255];

pub struct Canvas {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>, // RGBA, straight alpha
}

impl Canvas {
    pub fn new(w: u32, h: u32, fill: Rgba) -> Self {
        let mut px = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h {
            px.extend_from_slice(&fill);
        }
        Self { w, h, px }
    }

    /// Source-over blend of `c` with extra coverage `cov` (0..1).
    #[inline]
    fn blend(&mut self, x: i32, y: i32, c: Rgba, cov: f32) {
        if x < 0 || y < 0 || x as u32 >= self.w || y as u32 >= self.h {
            return;
        }
        let a = c[3] as f32 / 255.0 * cov.clamp(0.0, 1.0);
        if a <= 0.0 {
            return;
        }
        let i = ((y as u32 * self.w + x as u32) * 4) as usize;
        let da = self.px[i + 3] as f32 / 255.0;
        let oa = a + da * (1.0 - a);
        for (k, &src) in c.iter().take(3).enumerate() {
            let v = (src as f32 * a + self.px[i + k] as f32 * da * (1.0 - a)) / oa.max(1e-6);
            self.px[i + k] = v.round().clamp(0.0, 255.0) as u8;
        }
        self.px[i + 3] = (oa * 255.0).round() as u8;
    }

    /// Fill wherever `sdf(x, y) < 0` inside the box, anti-aliased over 1 px.
    fn fill_sdf(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, c: Rgba, sdf: impl Fn(f32, f32) -> f32) {
        let (xa, ya) = ((x0.floor() as i32).max(0), (y0.floor() as i32).max(0));
        let (xb, yb) = ((x1.ceil() as i32).min(self.w as i32), (y1.ceil() as i32).min(self.h as i32));
        for y in ya..yb {
            for x in xa..xb {
                let d = sdf(x as f32 + 0.5, y as f32 + 0.5);
                if d < 0.5 {
                    self.blend(x, y, c, 0.5 - d);
                }
            }
        }
    }

    pub fn round_rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, c: Rgba) {
        let r = r.min(w / 2.0).min(h / 2.0);
        let (cx, cy, hx, hy) = (x + w / 2.0, y + h / 2.0, w / 2.0 - r, h / 2.0 - r);
        self.fill_sdf(x - 1.0, y - 1.0, x + w + 1.0, y + h + 1.0, c, |px, py| {
            let qx = (px - cx).abs() - hx;
            let qy = (py - cy).abs() - hy;
            (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r
        });
    }

    pub fn ellipse(&mut self, cx: f32, cy: f32, rx: f32, ry: f32, c: Rgba) {
        self.fill_sdf(cx - rx - 1.0, cy - ry - 1.0, cx + rx + 1.0, cy + ry + 1.0, c, |px, py| {
            let (nx, ny) = ((px - cx) / rx, (py - cy) / ry);
            // First-order distance estimate in pixels.
            let k = (nx * nx + ny * ny).sqrt();
            (k - 1.0) * rx.min(ry)
        });
    }

    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, c: Rgba) {
        self.fill_sdf(cx - r - 1.0, cy - r - 1.0, cx + r + 1.0, cy + r + 1.0, c, |px, py| {
            ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - r
        });
    }

    pub fn ring(&mut self, cx: f32, cy: f32, r: f32, thick: f32, c: Rgba) {
        self.fill_sdf(cx - r - thick, cy - r - thick, cx + r + thick, cy + r + thick, c, |px, py| {
            (((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - r).abs() - thick / 2.0
        });
    }

    /// Round-capped line.
    pub fn line(&mut self, ax: f32, ay: f32, bx: f32, by: f32, thick: f32, c: Rgba) {
        let r = thick / 2.0;
        self.fill_sdf(ax.min(bx) - r - 1.0, ay.min(by) - r - 1.0, ax.max(bx) + r + 1.0, ay.max(by) + r + 1.0, c, |px, py| {
            let (pax, pay, bax, bay) = (px - ax, py - ay, bx - ax, by - ay);
            let t = ((pax * bax + pay * bay) / (bax * bax + bay * bay).max(1e-6)).clamp(0.0, 1.0);
            ((pax - bax * t).powi(2) + (pay - bay * t).powi(2)).sqrt() - r
        });
    }

    pub fn triangle(&mut self, p: [(f32, f32); 3], c: Rgba) {
        let (x0, x1) = (p.iter().map(|q| q.0).fold(f32::MAX, f32::min), p.iter().map(|q| q.0).fold(f32::MIN, f32::max));
        let (y0, y1) = (p.iter().map(|q| q.1).fold(f32::MAX, f32::min), p.iter().map(|q| q.1).fold(f32::MIN, f32::max));
        // Signed distance to each edge (positive outside, CCW-agnostic).
        let area = (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[2].0 - p[0].0) * (p[1].1 - p[0].1);
        let s = if area > 0.0 { 1.0 } else { -1.0 };
        self.fill_sdf(x0 - 1.0, y0 - 1.0, x1 + 1.0, y1 + 1.0, c, |px, py| {
            let mut d = f32::MIN;
            for i in 0..3 {
                let (a, b) = (p[i], p[(i + 1) % 3]);
                let (ex, ey) = (b.0 - a.0, b.1 - a.1);
                let len = (ex * ex + ey * ey).sqrt().max(1e-6);
                d = d.max(-s * (ex * (py - a.1) - ey * (px - a.0)) / len);
            }
            d
        });
    }
}

// ------------------------------------------------------------------ mascot

/// The UFOverlay mascot: a round little saucer with a sleepy alien in the dome.
/// `size` ≈ box width; `t` seconds drives bobbing, blinking and the rim lights.
pub fn mascot(c: &mut Canvas, cx: f32, cy: f32, size: f32, t: f32, beam: bool) {
    let s = size;
    let bob = (t * 2.4).sin() * s * 0.035;
    let cy = cy + bob;
    if beam {
        let a = (70.0 + 35.0 * (t * 3.0).sin()) as u8;
        c.triangle(
            [(cx - s * 0.16, cy + s * 0.12), (cx + s * 0.16, cy + s * 0.12), (cx, cy + s * 0.05)],
            [MINT[0], MINT[1], MINT[2], a / 3],
        );
        let top = cy + s * 0.1;
        let bot = cy + s * 0.48 - bob;
        c.fill_sdf(cx - s * 0.3, top, cx + s * 0.3, bot, [MINT[0], MINT[1], MINT[2], a / 2], |px, py| {
            let k = ((py - top) / (bot - top)).clamp(0.0, 1.0);
            (px - cx).abs() - (s * 0.1 + s * 0.16 * k)
        });
    }
    // Dome (glass) with the alien inside.
    c.circle(cx, cy - s * 0.1, s * 0.25, [150, 240, 220, 235]);
    c.ellipse(cx, cy - s * 0.07, s * 0.19, s * 0.16, [110, 220, 150, 255]); // alien head
    let blink = (t % 3.7) < 0.13;
    for dx in [-1.0f32, 1.0] {
        let ex = cx + dx * s * 0.075;
        let ey = cy - s * 0.1;
        if blink {
            c.line(ex - s * 0.032, ey, ex + s * 0.032, ey, s * 0.016, INK);
        } else {
            c.ellipse(ex, ey, s * 0.036, s * 0.046, INK);
            c.circle(ex + s * 0.012, ey - s * 0.016, s * 0.013, [255, 255, 255, 255]);
        }
        c.ellipse(ex + dx * s * 0.03, ey + s * 0.055, s * 0.026, s * 0.014, BLUSH);
    }
    c.line(cx - s * 0.025, cy - s * 0.035, cx + s * 0.025, cy - s * 0.035, s * 0.012, [40, 90, 60, 255]); // smile-ish
    c.ellipse(cx - s * 0.1, cy - s * 0.24, s * 0.055, s * 0.025, [255, 255, 255, 120]); // glass shine
                                                                                        // Saucer.
    c.ellipse(cx, cy + s * 0.1, s * 0.44, s * 0.1, [120, 104, 200, 255]);
    c.ellipse(cx, cy + s * 0.07, s * 0.46, s * 0.11, LAVENDER);
    c.ellipse(cx, cy + s * 0.045, s * 0.28, s * 0.04, [215, 205, 255, 255]);
    // Rim lights chase around.
    let lit = ((t * 5.0) as i32).rem_euclid(5);
    for i in 0..5 {
        let fx = -0.32 + 0.16 * i as f32;
        let y = cy + s * 0.1;
        let col = if i == lit { BULB } else { [255, 214, 107, 120] };
        c.circle(cx + fx * s, y, s * 0.027, col);
    }
}

/// App icon: the mascot beaming down inside a night-sky rounded square.
/// Rendered offline into assets/ by the `make_icons` test.
#[allow(dead_code)]
pub fn app_icon(size: u32) -> Canvas {
    let n = size as f32;
    let mut c = Canvas::new(size, size, [0, 0, 0, 0]);
    let inset = (n * 0.03).max(0.5);
    c.round_rect(inset, inset, n - inset * 2.0, n - inset * 2.0, n * 0.24, [27, 30, 52, 255]);
    if size >= 48 {
        for &(x, y, r, col) in
            &[(0.18, 0.2, 0.012, TEXT), (0.82, 0.16, 0.016, MINT), (0.74, 0.34, 0.01, LAVENDER), (0.22, 0.62, 0.01, MINT)]
        {
            c.circle(n * x, n * y, (n * r).max(0.8), col);
        }
    }
    // Small sizes get a bigger, simpler saucer so it still reads.
    let scale = if size <= 24 { 1.0 } else { 0.86 };
    mascot(&mut c, n / 2.0, n * 0.43, n * scale, 0.35, size > 24);
    c
}

// ------------------------------------------------------------------- icons

fn icon_monitor(c: &mut Canvas, x: f32, y: f32, s: f32, col: Rgba) {
    c.round_rect(x, y, s, s * 0.68, s * 0.12, col);
    c.round_rect(x + s * 0.09, y + s * 0.09, s * 0.82, s * 0.5, s * 0.06, SKY_2);
    c.round_rect(x + s * 0.42, y + s * 0.68, s * 0.16, s * 0.14, 1.0, col);
    c.round_rect(x + s * 0.26, y + s * 0.8, s * 0.48, s * 0.1, s * 0.05, col);
}

fn icon_window(c: &mut Canvas, x: f32, y: f32, s: f32, col: Rgba) {
    c.round_rect(x, y + s * 0.08, s, s * 0.8, s * 0.12, col);
    c.round_rect(x + s * 0.08, y + s * 0.28, s * 0.84, s * 0.52, s * 0.06, SKY_2);
    for i in 0..3 {
        c.circle(x + s * (0.62 + 0.12 * i as f32), y + s * 0.18, s * 0.04, SKY_2);
    }
}

fn icon_keyboard(c: &mut Canvas, x: f32, y: f32, s: f32, col: Rgba) {
    c.round_rect(x, y + s * 0.18, s, s * 0.64, s * 0.12, col);
    for r in 0..2 {
        for k in 0..5 {
            c.round_rect(x + s * (0.1 + 0.165 * k as f32), y + s * (0.3 + 0.17 * r as f32), s * 0.12, s * 0.1, 1.5, SKY_2);
        }
    }
    c.round_rect(x + s * 0.25, y + s * 0.64, s * 0.5, s * 0.08, 1.5, SKY_2);
}

fn icon_play(c: &mut Canvas, x: f32, y: f32, s: f32, col: Rgba) {
    c.triangle([(x + s * 0.2, y + s * 0.1), (x + s * 0.2, y + s * 0.9), (x + s * 0.9, y + s * 0.5)], col);
}

fn icon_pin(c: &mut Canvas, x: f32, y: f32, s: f32, col: Rgba) {
    // Map pin: round head, pointed tail, small hole.
    c.circle(x + s * 0.5, y + s * 0.38, s * 0.3, col);
    c.triangle([(x + s * 0.25, y + s * 0.52), (x + s * 0.75, y + s * 0.52), (x + s * 0.5, y + s * 0.98)], col);
    c.circle(x + s * 0.5, y + s * 0.38, s * 0.11, SKY_2);
}

fn icon_gather(c: &mut Canvas, x: f32, y: f32, s: f32, col: Rgba) {
    // Three little panels converging on a dot.
    c.round_rect(x, y + s * 0.3, s * 0.26, s * 0.36, s * 0.05, col);
    c.round_rect(x + s * 0.74, y + s * 0.3, s * 0.26, s * 0.36, s * 0.05, col);
    c.round_rect(x + s * 0.32, y + s * 0.12, s * 0.36, s * 0.42, s * 0.05, col);
    c.circle(x + s * 0.5, y + s * 0.82, s * 0.1, col);
}

fn icon_close(c: &mut Canvas, x: f32, y: f32, s: f32, col: Rgba) {
    let t = s * 0.16;
    c.line(x + s * 0.2, y + s * 0.2, x + s * 0.8, y + s * 0.8, t, col);
    c.line(x + s * 0.8, y + s * 0.2, x + s * 0.2, y + s * 0.8, t, col);
}

fn icon_refresh(c: &mut Canvas, x: f32, y: f32, s: f32, col: Rgba) {
    let (cx, cy, r) = (x + s / 2.0, y + s / 2.0, s * 0.32);
    let th = s * 0.12;
    c.fill_sdf(x, y, x + s, y + s, col, |px, py| {
        let (dx, dy) = (px - cx, py - cy);
        let ring = ((dx * dx + dy * dy).sqrt() - r).abs() - th / 2.0;
        // Cut a gap in the upper-right quadrant.
        if dx > 0.0 && dy < 0.0 && dx > -dy * 0.3 {
            ring.max(2.0)
        } else {
            ring
        }
    });
    c.triangle([(cx + r * 0.2, cy - r - th * 1.6), (cx + r * 0.2, cy - r + th * 1.6), (cx + r + th * 0.8, cy - r)], col);
}

fn icon_gear(c: &mut Canvas, x: f32, y: f32, s: f32, col: Rgba, bg: Rgba) {
    let (cx, cy) = (x + s / 2.0, y + s / 2.0);
    // Six short, blunt teeth + body + hole: reads as a gear even when small.
    for i in 0..6 {
        let a = i as f32 * std::f32::consts::PI / 3.0;
        let (dx, dy) = (a.cos(), a.sin());
        c.line(cx + dx * s * 0.3, cy + dy * s * 0.3, cx + dx * s * 0.42, cy + dy * s * 0.42, s * 0.2, col);
    }
    c.circle(cx, cy, s * 0.34, col);
    c.circle(cx, cy, s * 0.15, bg);
}

fn icon_back(c: &mut Canvas, x: f32, y: f32, s: f32, col: Rgba) {
    let t = s * 0.14;
    c.line(x + s * 0.62, y + s * 0.2, x + s * 0.3, y + s * 0.5, t, col);
    c.line(x + s * 0.3, y + s * 0.5, x + s * 0.62, y + s * 0.8, t, col);
}

fn icon_minus(c: &mut Canvas, x: f32, y: f32, s: f32, col: Rgba) {
    c.line(x + s * 0.28, y + s * 0.5, x + s * 0.72, y + s * 0.5, s * 0.12, col);
}

fn icon_plus(c: &mut Canvas, x: f32, y: f32, s: f32, col: Rgba) {
    icon_minus(c, x, y, s, col);
    c.line(x + s * 0.5, y + s * 0.28, x + s * 0.5, y + s * 0.72, s * 0.12, col);
}

// ---------------------------------------------------------------- toolbar

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    Live,
    Keyboard,
    Pin,
    Close,
}

pub struct Button {
    pub x0: u32,
    pub x1: u32,
    pub action: Action,
}

pub const TB_W: u32 = 700;
pub const TB_H: u32 = 96;

pub struct ToolbarState {
    pub live: bool,
    pub kb_open: bool,
    /// Carried on a controller.
    pub pinned: bool,
}

/// Small pill under a panel (only while you point at it): 영상 · 키보드 · 손에 · 닫기.
pub fn toolbar(st: &ToolbarState) -> (Canvas, Vec<Button>) {
    let (w, h) = (TB_W as f32, TB_H as f32);
    let mut c = Canvas::new(TB_W, TB_H, [0, 0, 0, 0]);
    c.round_rect(0.0, 0.0, w, h, h / 2.0, SKY);
    let pad = 12.0;
    let bh = h - pad * 2.0;
    let mut labels: Vec<Label> = Vec::new();
    let mut buttons = Vec::new();
    let specs: [(f32, &str, bool, Action); 4] = [
        (188.0, tr("toolbar.video"), st.live, Action::Live),
        (188.0, tr("toolbar.keyboard"), st.kb_open, Action::Keyboard),
        (188.0, if st.pinned { tr("toolbar.pinned") } else { tr("toolbar.pin") }, st.pinned, Action::Pin),
        (bh, "", false, Action::Close),
    ];
    let mut x = pad;
    for (bw, name, on, action) in specs {
        let fill = if on { MINT } else { SKY_2 };
        let fg = if on { INK } else { TEXT };
        c.round_rect(x, pad, bw, bh, bh / 2.0, fill);
        let is = bh * 0.46;
        let iy = pad + (bh - is) / 2.0;
        match action {
            Action::Live => icon_play(&mut c, x + 26.0, iy, is, if on { INK } else { MINT }),
            Action::Keyboard => icon_keyboard(&mut c, x + 22.0, iy, is * 1.15, if on { INK } else { LAVENDER }),
            Action::Pin => icon_pin(&mut c, x + 26.0, iy, is, if on { INK } else { BULB }),
            Action::Close => icon_close(&mut c, x + (bw - is) / 2.0, iy, is, CLOSE),
        }
        if !name.is_empty() {
            let px = if name.chars().count() > 4 { 22 } else { 28 };
            labels.push(label((x + 30.0 + is) as i32, pad as i32, (bw - 44.0 - is) as i32, bh as i32, name, px, fg, Align::Center, true));
        }
        buttons.push(Button { x0: x as u32, x1: (x + bw) as u32, action });
        x += bw + 8.0;
    }
    text::draw_all(&mut c, &labels);
    (c, buttons)
}

// ------------------------------------------------------------------- menu

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MenuHit {
    Source(usize),
    Refresh,
    Gather,
    ToggleAll,
    Settings,
}

pub const MENU_W: u32 = 600;
/// Gear button in the menu header: (x, y, size).
const GEAR: (f32, f32, f32) = (MENU_W as f32 - 84.0, 30.0, 58.0);
const MENU_HEAD: f32 = 120.0;
const MENU_ROW: f32 = 54.0;
const MENU_SECTION: f32 = 40.0;
const MENU_FOOT: f32 = 84.0;

pub struct MenuLayout {
    pub h: u32,
    rows: Vec<(f32, f32, MenuHit)>,
}

/// Menu items: (label, is_monitor, already_open).
pub type MenuItem = (String, bool, bool);

impl MenuLayout {
    pub fn new(items: &[MenuItem]) -> Self {
        let mut rows = Vec::new();
        let mut y = MENU_HEAD;
        let mut last_kind: Option<bool> = None;
        for (i, (_, is_mon, _)) in items.iter().enumerate() {
            if last_kind != Some(*is_mon) {
                y += MENU_SECTION;
                last_kind = Some(*is_mon);
            }
            rows.push((y, y + MENU_ROW, MenuHit::Source(i)));
            y += MENU_ROW;
        }
        y += 18.0;
        for hit in [MenuHit::Refresh, MenuHit::Gather, MenuHit::ToggleAll] {
            rows.push((y, y + MENU_FOOT - 24.0, hit));
        }
        Self { h: (y + MENU_FOOT) as u32, rows }
    }

    pub fn hit(&self, x: u32, y: u32) -> Option<MenuHit> {
        let (x, y) = (x as f32, y as f32);
        if x >= GEAR.0 && x < GEAR.0 + GEAR.2 && y >= GEAR.1 && y < GEAR.1 + GEAR.2 {
            return Some(MenuHit::Settings);
        }
        for &(y0, y1, hit) in &self.rows {
            if y >= y0 && y < y1 {
                let third = (x / (MENU_W as f32 / 3.0)) as usize;
                return match hit {
                    MenuHit::Refresh if third != 0 => continue,
                    MenuHit::Gather if third != 1 => continue,
                    MenuHit::ToggleAll if third < 2 => continue,
                    h => Some(h),
                };
            }
        }
        None
    }
}

pub fn menu(items: &[MenuItem], hover: Option<MenuHit>, all_hidden: bool) -> (Canvas, MenuLayout) {
    let lay = MenuLayout::new(items);
    let w = MENU_W as f32;
    let mut c = Canvas::new(MENU_W, lay.h, [0, 0, 0, 0]);
    let mut labels = Vec::new();
    c.round_rect(0.0, 0.0, w, lay.h as f32, 34.0, SKY);
    mascot(&mut c, 78.0, 58.0, 104.0, 0.6, false);
    labels.push(label(146, 26, 400, 40, "UFOverlay", 30, TEXT, Align::Left, true));
    labels.push(label(146, 64, 400, 30, tr("menu.prompt"), 21, DIM, Align::Left, false));
    let gear_on = hover == Some(MenuHit::Settings);
    c.circle(GEAR.0 + GEAR.2 / 2.0, GEAR.1 + GEAR.2 / 2.0, GEAR.2 / 2.0, if gear_on { MINT } else { SKY_2 });
    icon_gear(&mut c, GEAR.0 + 13.0, GEAR.1 + 13.0, GEAR.2 - 26.0, if gear_on { INK } else { DIM }, if gear_on { MINT } else { SKY_2 });

    let mut last_kind: Option<bool> = None;
    for (i, (name, is_mon, open)) in items.iter().enumerate() {
        let &(y0, _, _) = &lay.rows[i];
        if last_kind != Some(*is_mon) {
            last_kind = Some(*is_mon);
            labels.push(label(
                32,
                (y0 - MENU_SECTION + 6.0) as i32,
                300,
                30,
                if *is_mon { tr("menu.monitors") } else { tr("menu.windows") },
                18,
                DIM,
                Align::Left,
                true,
            ));
        }
        let hovered = hover == Some(MenuHit::Source(i));
        if hovered {
            c.round_rect(14.0, y0 + 3.0, w - 28.0, MENU_ROW - 6.0, 18.0, SKY_3);
        }
        let icol = if *is_mon { LAVENDER } else { MINT };
        if *is_mon {
            icon_monitor(&mut c, 32.0, y0 + 14.0, 30.0, icol)
        } else {
            icon_window(&mut c, 32.0, y0 + 12.0, 30.0, icol)
        }
        labels.push(label(80, y0 as i32, (w - 140.0) as i32, MENU_ROW as i32, name.clone(), 21, TEXT, Align::Left, false));
        if *open {
            c.circle(w - 40.0, y0 + MENU_ROW / 2.0, 6.0, MINT);
        }
    }
    let &(fy, fy1, _) = lay.rows.iter().find(|r| r.2 == MenuHit::Refresh).unwrap();
    for (i, hit) in [MenuHit::Refresh, MenuHit::Gather, MenuHit::ToggleAll].into_iter().enumerate() {
        // 18 px side margins, 10 px gaps, three equal buttons.
        let bw = (w - 36.0 - 20.0) / 3.0;
        let x = 18.0 + i as f32 * (bw + 10.0);
        let on = hover == Some(hit);
        c.round_rect(x, fy, bw, fy1 - fy, (fy1 - fy) / 2.0, if on { MINT } else { SKY_2 });
        let fg = if on { INK } else { TEXT };
        let ic = if on { INK } else { MINT };
        let iy = fy + (fy1 - fy) / 2.0 - 13.0;
        let name = match hit {
            MenuHit::Refresh => {
                icon_refresh(&mut c, x + 18.0, iy, 26.0, ic);
                tr("menu.refresh")
            }
            MenuHit::Gather => {
                icon_gather(&mut c, x + 18.0, iy, 26.0, ic);
                tr("menu.gather")
            }
            _ => {
                if all_hidden {
                    tr("menu.show_all")
                } else {
                    tr("menu.hide_all")
                }
            }
        };
        let tx = if hit == MenuHit::ToggleAll { x as i32 } else { x as i32 + 40 };
        let tw = if hit == MenuHit::ToggleAll { bw as i32 } else { bw as i32 - 44 };
        labels.push(label(tx, fy as i32, tw, (fy1 - fy) as i32, name, 20, fg, Align::Center, true));
    }
    text::draw_all(&mut c, &labels);
    (c, lay)
}

// --------------------------------------------------------------- settings

pub enum Ctrl {
    /// (value text, can decrease, can increase)
    Num(String, bool, bool),
    Toggle(bool),
    Action(&'static str),
}

pub struct RowView {
    pub section: &'static str,
    pub label: &'static str,
    pub ctrl: Ctrl,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SetHit {
    Back,
    Minus(usize),
    Plus(usize),
    Toggle(usize),
    Action(usize),
    /// Over a row but not on its control (hover highlight only).
    Row(usize),
}

pub const SET_W: u32 = 600;
const SET_HEAD: f32 = 108.0;
const SET_ROW: f32 = 62.0;
const SET_SECTION: f32 = 42.0;
const BACK: (f32, f32, f32) = (24.0, 28.0, 56.0);

pub struct SettingsLayout {
    pub h: u32,
    rows: Vec<(f32, f32, u8)>, // y0, y1, kind: 0 num, 1 toggle, 2 action
}

impl SettingsLayout {
    pub fn new(rows: &[RowView]) -> Self {
        let mut out = Vec::new();
        let mut y = SET_HEAD;
        let mut last = "";
        for r in rows {
            if r.section != last {
                y += SET_SECTION;
                last = r.section;
            }
            let kind = match r.ctrl {
                Ctrl::Num(..) => 0,
                Ctrl::Toggle(_) => 1,
                Ctrl::Action(_) => 2,
            };
            out.push((y, y + SET_ROW, kind));
            y += SET_ROW;
        }
        Self { h: (y + 28.0) as u32, rows: out }
    }

    pub fn hit(&self, x: u32, y: u32) -> Option<SetHit> {
        let (x, y) = (x as f32, y as f32);
        let w = SET_W as f32;
        if x >= BACK.0 && x < BACK.0 + BACK.2 && y >= BACK.1 && y < BACK.1 + BACK.2 {
            return Some(SetHit::Back);
        }
        for (i, &(y0, y1, kind)) in self.rows.iter().enumerate() {
            if y < y0 || y >= y1 {
                continue;
            }
            return Some(match kind {
                0 if x >= w - 250.0 && x < w - 190.0 => SetHit::Minus(i),
                0 if x >= w - 88.0 && x < w - 28.0 => SetHit::Plus(i),
                1 if x >= w - 140.0 => SetHit::Toggle(i),
                2 if x >= w - 200.0 => SetHit::Action(i),
                _ => SetHit::Row(i),
            });
        }
        None
    }
}

pub fn settings(rows: &[RowView], hover: Option<SetHit>) -> (Canvas, SettingsLayout) {
    let lay = SettingsLayout::new(rows);
    let w = SET_W as f32;
    let mut c = Canvas::new(SET_W, lay.h, [0, 0, 0, 0]);
    let mut labels = Vec::new();
    c.round_rect(0.0, 0.0, w, lay.h as f32, 34.0, SKY);
    let back_on = hover == Some(SetHit::Back);
    c.circle(BACK.0 + BACK.2 / 2.0, BACK.1 + BACK.2 / 2.0, BACK.2 / 2.0, if back_on { MINT } else { SKY_2 });
    icon_back(&mut c, BACK.0 + 12.0, BACK.1 + 12.0, BACK.2 - 24.0, if back_on { INK } else { TEXT });
    labels.push(label(100, 30, 300, 52, tr("settings.title"), 30, TEXT, Align::Left, true));
    mascot(&mut c, w - 78.0, 54.0, 96.0, 2.0, false);

    let mut last = "";
    for (i, r) in rows.iter().enumerate() {
        let (y0, _, _) = lay.rows[i];
        if r.section != last {
            last = r.section;
            labels.push(label(32, (y0 - SET_SECTION + 8.0) as i32, 300, 30, r.section, 18, DIM, Align::Left, true));
        }
        let row_hover =
            matches!(hover, Some(SetHit::Minus(k) | SetHit::Plus(k) | SetHit::Toggle(k) | SetHit::Action(k) | SetHit::Row(k)) if k == i);
        if row_hover {
            c.round_rect(14.0, y0 + 3.0, w - 28.0, SET_ROW - 6.0, 18.0, [33, 37, 58, 200]);
        }
        labels.push(label(32, y0 as i32, 300, SET_ROW as i32, r.label, 22, TEXT, Align::Left, false));
        let cy = y0 + SET_ROW / 2.0;
        match &r.ctrl {
            Ctrl::Num(text, dec, inc) => {
                for (x, on_hit, enabled, plus) in
                    [(w - 250.0, hover == Some(SetHit::Minus(i)), *dec, false), (w - 88.0, hover == Some(SetHit::Plus(i)), *inc, true)]
                {
                    let fill = if on_hit && enabled { MINT } else { SKY_2 };
                    let fg = if !enabled {
                        SKY_3
                    } else if on_hit {
                        INK
                    } else {
                        TEXT
                    };
                    c.circle(x + 30.0, cy, 24.0, fill);
                    if plus {
                        icon_plus(&mut c, x + 10.0, cy - 20.0, 40.0, fg)
                    } else {
                        icon_minus(&mut c, x + 10.0, cy - 20.0, 40.0, fg)
                    }
                }
                labels.push(label((w - 190.0) as i32, y0 as i32, 102, SET_ROW as i32, text.clone(), 21, LAVENDER, Align::Center, true));
            }
            Ctrl::Toggle(on) => {
                let (tw, th) = (84.0, 44.0);
                let x = w - 28.0 - tw;
                c.round_rect(x, cy - th / 2.0, tw, th, th / 2.0, if *on { MINT } else { SKY_3 });
                let kx = if *on { x + tw - th / 2.0 } else { x + th / 2.0 };
                c.circle(kx, cy, th / 2.0 - 5.0, if *on { [255, 255, 255, 255] } else { DIM });
            }
            Ctrl::Action(name) => {
                let on_hit = hover == Some(SetHit::Action(i));
                let (bw, bh) = (150.0, 44.0);
                let x = w - 28.0 - bw;
                c.round_rect(x, cy - bh / 2.0, bw, bh, bh / 2.0, if on_hit { MINT } else { SKY_2 });
                labels.push(label(
                    x as i32,
                    (cy - bh / 2.0) as i32,
                    bw as i32,
                    bh as i32,
                    *name,
                    20,
                    if on_hit { INK } else { TEXT },
                    Align::Center,
                    true,
                ));
            }
        }
    }
    text::draw_all(&mut c, &labels);
    (c, lay)
}

// ----------------------------------------------------------------- cursor

pub const CURSOR_PX: u32 = 64;

pub fn cursor() -> Canvas {
    let n = CURSOR_PX as f32;
    let mut c = Canvas::new(CURSOR_PX, CURSOR_PX, [0, 0, 0, 0]);
    let m = n / 2.0;
    c.circle(m, m, m * 0.62, [124, 242, 201, 60]);
    c.ring(m, m, m * 0.7, m * 0.36, [20, 22, 36, 170]);
    c.ring(m, m, m * 0.7, m * 0.2, [255, 255, 255, 255]);
    c.circle(m, m, m * 0.12, MINT);
    c
}

// --------------------------------------------------------------- keyboard

pub use crate::kbd::KeyKind;
use crate::kbd::{KeyDef, Layout};

/// A layout key placed on the grid (units).
pub struct Key {
    pub def: KeyDef,
    pub x: f32,
    pub y: f32,
}

pub const KB_UNIT: u32 = 80;
pub const KB_PAD: u32 = 18;
pub const KB_COLS: f32 = crate::kbd::ROW_UNITS;
pub const KB_ROWS: u32 = 5;
pub const KB_W: u32 = (KB_COLS as u32) * KB_UNIT + KB_PAD * 2;
pub const KB_H: u32 = KB_ROWS * KB_UNIT + KB_PAD * 2;

pub fn keyboard_keys(layout: &Layout) -> Vec<Key> {
    let mut keys = Vec::new();
    for (ry, row) in layout.rows.iter().take(KB_ROWS as usize).enumerate() {
        let mut x = 0.0;
        for def in row {
            keys.push(Key { def: def.clone(), x, y: ry as f32 });
            x += def.width;
        }
    }
    keys
}

pub fn key_at(keys: &[Key], px: f32, py: f32) -> Option<&Key> {
    let (ux, uy) = ((px - KB_PAD as f32) / KB_UNIT as f32, (py - KB_PAD as f32) / KB_UNIT as f32);
    keys.iter().find(|k| ux >= k.x && ux < k.x + k.def.width && uy >= k.y && uy < k.y + 1.0)
}

pub struct KbMods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// Draws the keyboard for the focused window's IME. Keys with a legend (e.g.
/// 두벌식 jamo) show it big while native input is on and small otherwise; the
/// IME key shows the current mode (한/영, あ/A, …).
pub fn keyboard(keys: &[Key], layout: &Layout, m: &KbMods, ime: ImeState) -> Canvas {
    let mut c = Canvas::new(KB_W, KB_H, [0, 0, 0, 0]);
    c.round_rect(0.0, 0.0, KB_W as f32, KB_H as f32, 30.0, SKY);
    let mut labels = Vec::new();
    let u = KB_UNIT as f32;
    let face = layout.font;
    let with_face = |mut l: Label, f: Option<&'static str>| {
        l.face = f;
        l
    };
    for k in keys {
        let d = &k.def;
        let x = KB_PAD as f32 + k.x * u + 4.0;
        let y = KB_PAD as f32 + k.y * u + 4.0;
        let (w, h) = (d.width * u - 8.0, u - 8.0);
        let on = match d.kind {
            KeyKind::Shift => m.shift,
            KeyKind::Ctrl => m.ctrl,
            KeyKind::Alt => m.alt,
            KeyKind::Ime => ime.native,
            _ => false,
        };
        let (fill, fg) = if on {
            (if d.kind == KeyKind::Ime { LAVENDER } else { MINT }, INK)
        } else {
            match d.kind {
                KeyKind::Ime => (SKY_2, LAVENDER),
                KeyKind::Close => ([72, 40, 62, 255], CLOSE),
                KeyKind::Vk(..) if d.width <= 1.0 => (SKY_3, TEXT),
                _ => (SKY_2, TEXT),
            }
        };
        c.round_rect(x, y, w, h, 16.0, fill);
        let (xi, yi, wi, hi) = (x as i32, y as i32, w as i32, h as i32);
        let latin = if m.shift { d.shifted } else { d.label };
        let legend = if m.shift { d.legend_shifted } else { d.legend };
        if d.kind == KeyKind::Ime {
            let (name, on_txt, off_txt) = layout.ime.unwrap_or((d.label, d.label, d.label));
            let mode = if ime.native { on_txt } else { off_txt };
            labels.push(with_face(label(xi, yi + 10, wi, hi - 10, mode, 34, fg, Align::Center, true), face));
            labels.push(with_face(label(xi + 10, yi + 4, wi - 20, 22, name, 15, if on { INK } else { DIM }, Align::Left, false), face));
        } else if !legend.is_empty() {
            if ime.native && layout.legend_big_when_ime {
                labels.push(with_face(label(xi, yi + 6, wi, hi - 6, legend, 32, fg, Align::Center, false), face));
                labels.push(label(xi + 10, yi + 4, 30, 24, latin, 16, DIM, Align::Left, false));
            } else {
                labels.push(label(xi, yi + 6, wi, hi - 6, latin, 30, fg, Align::Center, false));
                labels.push(with_face(label(xi + 10, yi + 4, 30, 24, legend, 16, DIM, Align::Left, false), face));
            }
        } else {
            let t = if d.kind == KeyKind::Close { tr("keyboard.close") } else { latin };
            // CJK fonts draw "\" as ¥ / ₩; keep the real backslash.
            let f = if d.custom {
                face
            } else if matches!(t, "\\" | "|") {
                Some("Segoe UI")
            } else {
                None
            };
            let px = if t.chars().count() > 2 { 22 } else { 30 };
            labels.push(with_face(label(xi, yi, wi, hi, t, px, fg, Align::Center, false), f));
        }
    }
    text::draw_all(&mut c, &labels);
    c
}

// ------------------------------------------------------------------ wrist

pub struct WristInfo {
    pub time: String,
    pub batteries: Vec<(char, Option<f32>, bool)>, // (label, 0..1, charging)
}

pub const WRIST_W: u32 = 360;
pub const WRIST_H: u32 = 140;

/// Static layer (background, clock, batteries); the mascot is drawn on top per frame.
pub fn wrist_base(info: &WristInfo) -> Canvas {
    let (w, h) = (WRIST_W as f32, WRIST_H as f32);
    let mut c = Canvas::new(WRIST_W, WRIST_H, [0, 0, 0, 0]);
    c.round_rect(0.0, 0.0, w, h, h / 2.0, SKY);
    let mut labels = vec![label(140, 10, 210, 78, info.time.clone(), 60, TEXT, Align::Left, true)];
    let mut x = 142.0;
    for (name, pct, charging) in &info.batteries {
        let y = 96.0;
        let v = pct.unwrap_or(0.0).clamp(0.0, 1.0);
        let col = if *charging {
            MINT
        } else if pct.is_some() && v <= 0.2 {
            WARN
        } else {
            LAVENDER
        };
        c.round_rect(x, y, 30.0, 16.0, 5.0, SKY_3);
        if pct.is_some() {
            c.round_rect(x + 2.0, y + 2.0, 26.0 * v, 12.0, 4.0, col);
        }
        c.round_rect(x + 30.0, y + 5.0, 3.0, 6.0, 1.0, SKY_3);
        labels.push(label(x as i32 + 38, y as i32 - 6, 30, 28, name.to_string(), 17, DIM, Align::Left, true));
        x += 72.0;
    }
    text::draw_all(&mut c, &labels);
    c
}

pub fn wrist_frame(base: &Canvas, t: f32) -> Canvas {
    let mut c = Canvas { w: base.w, h: base.h, px: base.px.clone() };
    mascot(&mut c, 72.0, 64.0, 118.0, t, true);
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_hit_rows_and_footer() {
        let items: Vec<MenuItem> = vec![("모니터 1".into(), true, false), ("모니터 2".into(), true, false), ("창".into(), false, true)];
        let lay = MenuLayout::new(&items);
        let y_of = |i: usize| lay.rows[i].0 as u32 + 5;
        assert_eq!(lay.hit(100, y_of(0)), Some(MenuHit::Source(0)));
        assert_eq!(lay.hit(100, y_of(2)), Some(MenuHit::Source(2)));
        let fy = lay.rows[3].0 as u32 + 5;
        assert_eq!(lay.hit(10, fy), Some(MenuHit::Refresh));
        assert_eq!(lay.hit(MENU_W / 2, fy), Some(MenuHit::Gather));
        assert_eq!(lay.hit(MENU_W - 10, fy), Some(MenuHit::ToggleAll));
        assert!(lay.h > fy);
        // Section header gap between monitors and windows is not a row.
        assert_eq!(lay.hit(100, lay.rows[2].0 as u32 - 10), None);
    }

    #[test]
    fn settings_controls_hit() {
        let rows = vec![
            RowView { section: "a", label: "n", ctrl: Ctrl::Num("1".into(), true, true) },
            RowView { section: "b", label: "t", ctrl: Ctrl::Toggle(true) },
        ];
        let lay = SettingsLayout::new(&rows);
        let y0 = lay.rows[0].0 as u32 + 30;
        assert_eq!(lay.hit(SET_W - 220, y0), Some(SetHit::Minus(0)));
        assert_eq!(lay.hit(SET_W - 60, y0), Some(SetHit::Plus(0)));
        assert_eq!(lay.hit(100, y0), Some(SetHit::Row(0)));
        assert_eq!(lay.hit(SET_W - 60, lay.rows[1].0 as u32 + 30), Some(SetHit::Toggle(1)));
        assert_eq!(lay.hit(40, 50), Some(SetHit::Back));
    }

    #[test]
    fn toolbar_buttons_hit_regions_dont_overlap() {
        let (_, b) = toolbar(&ToolbarState { live: false, kb_open: false, pinned: false });
        assert_eq!(b.len(), 4);
        for w in b.windows(2) {
            assert!(w[0].x1 <= w[1].x0);
        }
        assert!(b.last().unwrap().x1 <= TB_W);
    }

    #[test]
    fn keyboard_keys_hit() {
        let keys = keyboard_keys(crate::kbd::for_lcid(0x12));
        let q = key_at(&keys, (KB_PAD + KB_UNIT * 3 / 2 + 5) as f32, (KB_PAD + KB_UNIT + 5) as f32).unwrap();
        assert_eq!((q.def.label, q.def.legend), ("q", "ㅂ"));
    }

    #[test]
    fn mascot_draws_inside_canvas() {
        let base = wrist_base(&WristInfo { time: "12:34".into(), batteries: vec![('L', Some(0.8), false)] });
        let f = wrist_frame(&base, 1.0);
        assert!(f.px.chunks(4).filter(|p| p[3] > 0).count() > 1000);
    }

    fn write_png(path: &std::path::Path, c: &Canvas) {
        std::fs::write(path, crate::png::encode(c, Some(90))).unwrap();
    }

    /// Regenerates the README screenshots in docs/images
    /// (`cargo test docs_images -- --ignored`, then `python tools/compress_png.py docs/images`).
    #[test]
    #[ignore]
    fn docs_images() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs").join("images");
        std::fs::create_dir_all(&dir).unwrap();
        let save = |name: &str, c: &Canvas| std::fs::write(dir.join(name), crate::png::encode(c, None)).unwrap();
        crate::i18n::set("en");
        let items: Vec<MenuItem> = vec![
            ("Monitor 1  ·  2560×1440".into(), true, true),
            ("Monitor 2  ·  1920×1080".into(), true, false),
            ("Discord".into(), false, false),
            ("Spotify Premium".into(), false, false),
            ("YouTube - Mozilla Firefox".into(), false, true),
        ];
        save("menu.png", &menu(&items, Some(MenuHit::Source(1)), false).0);
        save("toolbar.png", &toolbar(&ToolbarState { live: true, kb_open: false, pinned: false }).0);
        let rows: Vec<RowView> = crate::settings::items()
            .into_iter()
            .take(8)
            .map(|it| {
                let cfg = crate::config::Config::default();
                let ctrl = match it.kind {
                    crate::settings::Kind::Num { .. } => {
                        let v = crate::settings::num(&cfg, it.id);
                        Ctrl::Num(crate::settings::format(it.id, v), true, true)
                    }
                    crate::settings::Kind::Toggle => Ctrl::Toggle(true),
                    crate::settings::Kind::Action(a) => Ctrl::Action(tr(a)),
                };
                RowView { section: tr(it.section), label: tr(it.label), ctrl }
            })
            .collect();
        save("settings.png", &settings(&rows, Some(SetHit::Plus(2))).0);
        let mods = KbMods { shift: false, ctrl: false, alt: false };
        for (name, lcid, native) in [("keyboard_ko.png", 0x12u16, true), ("keyboard_ja.png", 0x11, true)] {
            let layout = crate::kbd::for_lcid(lcid);
            save(name, &keyboard(&keyboard_keys(layout), layout, &mods, ImeState { lcid, native }));
        }
        let base = wrist_base(&WristInfo {
            time: "21:47".into(),
            batteries: vec![('L', Some(0.82), false), ('R', Some(0.64), false), ('H', Some(0.9), true)],
        });
        save("wrist.png", &wrist_frame(&base, 0.4));
        crate::i18n::set("ko");
    }

    /// Regenerates assets/ufoverlay.ico + .png (`cargo test make_icons -- --ignored`).
    #[test]
    #[ignore]
    fn make_icons() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
        std::fs::create_dir_all(&dir).unwrap();
        let sizes = [16u32, 24, 32, 48, 64, 128, 256];
        let images: Vec<(u32, Vec<u8>)> = sizes.iter().map(|&n| (n, crate::png::encode(&app_icon(n), None))).collect();
        std::fs::write(dir.join("ufoverlay.ico"), crate::png::ico(&images)).unwrap();
        std::fs::write(dir.join("ufoverlay.png"), crate::png::encode(&app_icon(256), None)).unwrap();
        if let Ok(d) = std::env::var("UFO_DUMP") {
            for n in [16u32, 32, 256] {
                std::fs::write(std::path::Path::new(&d).join(format!("icon_{n}.png")), crate::png::encode(&app_icon(n), Some(200)))
                    .unwrap();
            }
        }
    }

    #[test]
    #[ignore]
    fn dump_ui() {
        let dir = std::path::PathBuf::from(std::env::var("UFO_DUMP").unwrap_or_else(|_| ".".into()));
        let base = wrist_base(&WristInfo {
            time: "14:27".into(),
            batteries: vec![('L', Some(0.82), false), ('R', Some(0.15), false), ('H', Some(0.6), true)],
        });
        for (i, t) in [0.0f32, 0.4, 3.75].iter().enumerate() {
            write_png(&dir.join(format!("wrist_{i}.png")), &wrist_frame(&base, *t));
        }
        let items: Vec<MenuItem> = vec![
            ("모니터 1  ·  2560×1440".into(), true, true),
            ("모니터 2  ·  1920×1080".into(), true, false),
            ("Discord".into(), false, false),
            ("카카오톡".into(), false, false),
            ("Claude".into(), false, true),
        ];
        write_png(&dir.join("menu.png"), &menu(&items, Some(MenuHit::Source(1)), false).0);
        write_png(&dir.join("toolbar.png"), &toolbar(&ToolbarState { live: true, kb_open: false, pinned: true }).0);
        let mods = KbMods { shift: false, ctrl: false, alt: false };
        for (name, lcid, native) in [("ko_han", 0x12u16, true), ("ko_eng", 0x12, false), ("ja_on", 0x11, true), ("en", 0x09, false)] {
            let layout = crate::kbd::for_lcid(lcid);
            write_png(
                &dir.join(format!("keyboard_{name}.png")),
                &keyboard(&keyboard_keys(layout), layout, &mods, ImeState { lcid, native }),
            );
        }
        for code in ["ja", "en"] {
            crate::i18n::set(code);
            write_png(&dir.join(format!("menu_{code}.png")), &menu(&items, None, false).0);
            write_png(&dir.join(format!("toolbar_{code}.png")), &toolbar(&ToolbarState { live: false, kb_open: true, pinned: false }).0);
        }
        crate::i18n::set("ko");
        write_png(&dir.join("cursor.png"), &cursor());
        let rows = vec![
            RowView { section: "화면", label: "가만히 볼 때", ctrl: Ctrl::Num("15 fps".into(), true, true) },
            RowView { section: "화면", label: "가리킬 때", ctrl: Ctrl::Num("144 fps".into(), true, false) },
            RowView { section: "손목", label: "24시간제", ctrl: Ctrl::Toggle(true) },
            RowView { section: "손목", label: "시작 애니메이션", ctrl: Ctrl::Toggle(false) },
            RowView { section: "손목", label: "시계 위치", ctrl: Ctrl::Action("처음대로") },
        ];
        write_png(&dir.join("settings.png"), &settings(&rows, Some(SetHit::Plus(0))).0);
        write_png(&dir.join("menu_gear.png"), &menu(&items, Some(MenuHit::Settings), false).0);
    }
}
