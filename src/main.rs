//! UFOverlay — Ultra Fast Overlay for SteamVR.
//!
//! World-space desktop/window panels you grab and place with the controllers.
//!
//! Design rules:
//!  * No engine, no browser, no UI framework. One thread, one D3D11 device.
//!  * Pixels never leave the GPU: Windows.Graphics.Capture → CopySubresource →
//!    SetOverlayTexture. Unchanged sources cost nothing (frames are events).
//!  * Adaptive rate: fps_min when nobody looks, fps_max while you interact.
//!  * Pointer hit-testing is local math on transforms we already know — no IPC.
//!  * Static images (toolbar, menu, wrist, keyboard, cursor) are redrawn only
//!    on change.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod config;
mod elevate;
mod i18n;
mod ime;
mod input;
mod kbd;
mod layout;
mod math;
mod png;
mod settings;
mod sources;
mod text;
mod tinytoml;
mod ui;
mod vr;

use std::fs::File;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use capture::{Capture, Gpu, Grab};
use config::Config;
use math::{ray_quad, V3};
use sources::Source;
use vr::{ev, Event, Handle, Mat34, Pose, Vr, INVALID_DEVICE};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

static LOG: Mutex<Option<File>> = Mutex::new(None);

macro_rules! log {
    ($($t:tt)*) => {{
        let line = format!($($t)*);
        eprintln!("{line}");
        if let Ok(mut g) = LOG.lock() {
            if let Some(f) = g.as_mut() { let _ = writeln!(f, "{line}"); }
        }
    }};
}

const APP_KEY: &str = "ufo.overlay";
const LEFT: usize = 0;
const RIGHT: usize = 1;
const MAX_POSES: usize = 16;

const TICK_ENGAGED: Duration = Duration::from_millis(11);
const TICK_VISIBLE: Duration = Duration::from_millis(33);
const TICK_IDLE: Duration = Duration::from_millis(50);
/// Toolbar lingers this long after the pointer leaves its panel.
const TOOLBAR_LINGER: Duration = Duration::from_millis(1500);

const MENU_W_M: f32 = 0.40;
/// Wrist animation frame interval (only while you look at it).
const WRIST_FRAME: Duration = Duration::from_millis(83);
const POP: Duration = Duration::from_millis(220);
/// Boot flight: fly in, hover + beam, fade.
const BOOT_FLY: f32 = 1.1;
const BOOT_BEAM_AT: f32 = 1.25;
const BOOT_END: f32 = 1.75;
const BOOT_FRAME: Duration = Duration::from_millis(16);
const KB_W_M: f32 = 0.56;
const SET_W_M: f32 = 0.40;

fn exe_dir() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())).unwrap_or_default()
}

fn edge(b: (bool, bool)) -> (bool, bool) {
    (b.0 && b.1, !b.0 && b.1) // (pressed now, released now)
}

fn mat_to_12(m: &Mat34) -> [f32; 12] {
    let mut o = [0.0; 12];
    for r in 0..3 {
        for c in 0..4 {
            o[r * 4 + c] = m.m[r][c];
        }
    }
    o
}

fn mat_from_12(a: &[f32; 12]) -> Mat34 {
    let mut m = Mat34::default();
    for r in 0..3 {
        for c in 0..4 {
            m.m[r][c] = a[r * 4 + c];
        }
    }
    m
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Target {
    Panel(u64, f32, f32),
    Toolbar(u64, f32, f32),
    Menu(f32, f32),
    Keyboard(f32, f32),
    Settings(f32, f32),
    Wrist,
}

impl Target {
    fn same_object(&self, o: &Target) -> bool {
        match (self, o) {
            (Target::Panel(a, ..), Target::Panel(b, ..)) | (Target::Toolbar(a, ..), Target::Toolbar(b, ..)) => a == b,
            (Target::Menu(..), Target::Menu(..))
            | (Target::Keyboard(..), Target::Keyboard(..))
            | (Target::Settings(..), Target::Settings(..))
            | (Target::Wrist, Target::Wrist) => true,
            _ => false,
        }
    }
    fn panel_id(&self) -> Option<u64> {
        match self {
            Target::Panel(id, ..) | Target::Toolbar(id, ..) => Some(*id),
            _ => None,
        }
    }
}

/// Default wrist placement in the aim (tip) pose frame: floating over the back
/// of the hand, text reading like a watch face. Mirrored for the right hand.
fn wrist_default_local(hand: usize) -> Mat34 {
    if hand == LEFT {
        Mat34::from_axes(V3(0., 0., -1.), V3(0., -1., 0.), V3(-1., 0., 0.), V3(-0.06, -0.03, 0.10))
    } else {
        Mat34::from_axes(V3(0., 0., 1.), V3(0., -1., 0.), V3(1., 0., 0.), V3(0.06, -0.03, 0.10))
    }
}

/// Exponential pointer filter that follows fast swings but calms small
/// tremors: the slower the aim moves, the more it is smoothed.
fn smooth_pose(prev: &Mat34, cur: &Mat34, dt: f32, strength: u32) -> Mat34 {
    let ang_speed = prev.forward().dot(cur.forward()).clamp(-1.0, 1.0).acos() / dt.max(1e-3);
    let a_min = if strength >= 2 { 0.10 } else { 0.28 };
    let a = (a_min + ang_speed / 2.5).clamp(a_min, 1.0);
    // Make the filter independent of tick rate (11 ms reference).
    let a = 1.0 - (1.0 - a).powf(dt / 0.011);
    let lerp = |p: V3, c: V3| p + (c - p) * a;
    let z = lerp(prev.col(2), cur.col(2)).norm();
    let x = lerp(prev.col(1), cur.col(1)).cross(z).norm();
    let y = z.cross(x);
    Mat34::from_axes(x, y, z, lerp(prev.pos(), cur.pos()))
}

/// Two GPU textures per CPU-drawn overlay, used alternately. Uploading with
/// SetOverlayRaw makes SteamVR swap the image asynchronously and the overlay
/// blinks for a frame; a texture we own and re-submit never goes blank.
#[derive(Default)]
struct ImageSlot {
    tex: [Option<(windows::Win32::Graphics::Direct3D11::ID3D11Texture2D, u32, u32)>; 2],
    next: usize,
}

/// Pop-in animation of an overlay's width (and alpha).
struct Tween {
    handle: Handle,
    width: f32,
    alpha: f32,
    start: Instant,
}

fn ease_out_back(t: f32) -> f32 {
    let (c1, c3) = (1.70158, 2.70158);
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}

/// Startup flourish, played *after* everything is ready (never delays boot).
struct Boot {
    handle: Handle,
    start: Instant,
    from: V3,
    next_frame: Instant,
    beamed: bool,
}

fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Held {
    Panel(u64),
    Keyboard,
    Wrist,
}

struct Panel {
    id: u64,
    handle: Handle,
    src: Source,
    cap: Capture,
    width_m: f32,
    world: Mat34,
    /// (hand, panel-in-controller transform) while grabbed.
    held: Option<(usize, Mat34)>,
    /// (hand, panel-in-controller transform) while carried on a controller.
    attached: Option<(usize, Mat34)>,
    live: bool,
    boost_until: Instant,
    last_submit: Instant,
    dirty: bool,
    tb: Handle,
    tb_on: bool,
    tb_until: Instant,
    buttons: Vec<ui::Button>,
    /// (live, kb_open, attached) the toolbar image currently shows.
    tb_drawn: Option<(bool, bool, bool)>,
}

impl Panel {
    fn height_m(&self) -> f32 {
        self.width_m * self.cap.h as f32 / self.cap.w as f32
    }
    fn fps(&self, cfg: &Config, now: Instant) -> u32 {
        if self.live || now < self.boost_until {
            cfg.fps_max
        } else {
            cfg.fps_min
        }
    }
    fn tb_w(&self) -> f32 {
        (self.width_m * 0.3).clamp(0.16, 0.34)
    }
    fn tb_h(&self) -> f32 {
        self.tb_w() * ui::TB_H as f32 / ui::TB_W as f32
    }
    /// Toolbar transform relative to the panel: just below its bottom edge.
    fn tb_offset(&self) -> Mat34 {
        Mat34::pitch_translate(0.0, [0.0, -(self.height_m() / 2.0 + self.tb_h() / 2.0 + 0.012), 0.002])
    }
}

struct Hand {
    src: u64,
    dev: u32,
    raw: Option<Mat34>,
    aim: Option<Mat34>,
    trig: (bool, bool),
    grip: (bool, bool),
    rclick: (bool, bool),
    stick: (f32, f32),
    target: Option<Target>,
    dist: f32,
    holding: Option<Held>,
    mouse_down: Option<bool>, // Some(right?)
    scroll_acc: f32,
    cursor: Handle,
    cursor_on: bool,
    cursor_w: f32,
    /// Hold the OS cursor still right after a click so jitter can't drag.
    freeze_until: Instant,
}

struct Zoom {
    panel: u64,
    hand: usize,
    d0: f32,
    w0: f32,
}

struct Actions {
    set: u64,
    aim: u64,
    click: u64,
    grab: u64,
    rclick: u64,
    stick: u64,
    haptic: u64,
}

struct Keyboard {
    handle: Handle,
    open: bool,
    tf: Mat34,
    held: Option<(usize, Mat34)>,
    keys: Vec<ui::Key>,
    mods: ui::KbMods,
    /// Panel whose KB button opened it (its toolbar shows KB as active).
    for_panel: Option<u64>,
    /// Layout picked for the focused window's input language.
    layout: &'static kbd::Layout,
    /// IME of the focused window (language + 한/영, あ/A), polled while open.
    ime: ime::ImeState,
    ime_check_at: Instant,
}

struct App {
    vr: Vr,
    gpu: Gpu,
    cfg: Config,
    ini: PathBuf,
    act: Actions,
    hands: [Hand; 2],
    poses: [Pose; MAX_POSES],
    panels: Vec<Panel>,
    next_id: u64,
    all_hidden: bool,
    zoom: Option<Zoom>,
    mouse_hand: Option<usize>,
    last_mouse: (f32, f32),

    menu: Handle,
    menu_open: bool,
    menu_tf: Mat34,
    menu_rows: Vec<Source>,
    menu_layout: ui::MenuLayout,
    menu_hover: Option<ui::MenuHit>,

    kb: Keyboard,

    settings: Handle,
    settings_open: bool,
    settings_layout: ui::SettingsLayout,
    settings_hover: Option<ui::SetHit>,
    /// SteamVR's own auto-launch flag for our app key (read when the panel opens).
    autostart: bool,

    wrist: Option<Handle>,
    /// Placement relative to the left controller; computed from the aim pose
    /// the first time we see the hand, unless the user saved one.
    wrist_rel: Option<Mat34>,
    wrist_held: Option<(usize, Mat34)>,
    wrist_key: String,
    wrist_base: Option<ui::Canvas>,
    wrist_vis: bool,
    wrist_alpha: f32,
    wrist_next_frame: Instant,
    t0: Instant,
    tweens: Vec<Tween>,
    images: std::collections::HashMap<Handle, ImageSlot>,
    /// Keep the wrist shown (e.g. right after the boot beam) until this instant.
    wrist_force_until: Instant,
    boot: Option<Boot>,
    boot_pending: bool,
    battery_at: Instant,
    batteries: Vec<(char, Option<f32>, bool)>,

    last_input: Instant,
    layout_path: PathBuf,
    layout_dirty: bool,
    /// Pending "create admin task" job (it waits on a UAC prompt off the VR thread).
    admin_job: Option<std::sync::mpsc::Receiver<Result<(), String>>>,
}

impl App {
    fn new(cfg: Config, ini: PathBuf) -> Result<Self, String> {
        let vr = Vr::init()?;
        let wake = unsafe { CreateEventW(None, false, false, None) }.map_err(|e| e.to_string())?;
        let gpu = Gpu::new(vr.dxgi_adapter_index(), wake).map_err(|e| format!("D3D11 init: {e}"))?;

        // SteamVR Input: manifest + default bindings live next to the exe.
        let dir = exe_dir().join("input");
        let _ = std::fs::create_dir_all(&dir);
        for (name, body) in [
            ("actions.json", include_str!("../input/actions.json")),
            ("bindings_oculus_touch.json", include_str!("../input/bindings_oculus_touch.json")),
            ("bindings_knuckles.json", include_str!("../input/bindings_knuckles.json")),
        ] {
            let _ = std::fs::write(dir.join(name), body);
        }
        vr.set_action_manifest(&dir.join("actions.json").to_string_lossy())?;
        let act = Actions {
            set: vr.action_set("/actions/main"),
            aim: vr.action("/actions/main/in/aim"),
            click: vr.action("/actions/main/in/click"),
            grab: vr.action("/actions/main/in/grab"),
            rclick: vr.action("/actions/main/in/rclick"),
            stick: vr.action("/actions/main/in/stick"),
            haptic: vr.action("/actions/main/out/haptic"),
        };

        let cursor_img = ui::cursor();
        let mk_hand = |i: usize| -> Result<Hand, String> {
            let cursor = vr.create_overlay(&format!("ufo.cursor.{i}"), "UFO cursor")?;
            vr.set_width(cursor, 0.015);
            vr.set_sort_order(cursor, 30);
            Ok(Hand {
                src: vr.input_source(if i == LEFT { "/user/hand/left" } else { "/user/hand/right" }),
                dev: INVALID_DEVICE,
                raw: None,
                aim: None,
                trig: (false, false),
                grip: (false, false),
                rclick: (false, false),
                stick: (0., 0.),
                target: None,
                dist: 0.,
                holding: None,
                mouse_down: None,
                scroll_acc: 0.,
                cursor,
                cursor_on: false,
                cursor_w: 0.015,
                freeze_until: Instant::now(),
            })
        };
        let hands = [mk_hand(LEFT)?, mk_hand(RIGHT)?];

        let menu = vr.create_overlay("ufo.menu", "UFO menu")?;
        vr.set_width(menu, MENU_W_M);
        vr.set_sort_order(menu, 15);

        let settings_handle = vr.create_overlay("ufo.settings", "UFO settings")?;
        vr.set_width(settings_handle, SET_W_M);
        vr.set_sort_order(settings_handle, 15);

        let kb_handle = vr.create_overlay("ufo.keyboard", "UFO keyboard")?;
        vr.set_width(kb_handle, KB_W_M);
        vr.set_sort_order(kb_handle, 16);

        let wrist = if cfg.wrist {
            let h = vr.create_overlay("ufo.wrist", "UFO wrist")?;
            vr.set_width(h, cfg.wrist_width_m);
            vr.set_sort_order(h, 10);
            Some(h)
        } else {
            None
        };
        let wrist_rel = cfg.wrist_pose.as_ref().map(mat_from_12);
        let cfg_boot_anim = cfg.boot_anim;
        let now = Instant::now();
        let mut app = App {
            vr,
            gpu,
            cfg,
            ini,
            act,
            hands,
            poses: [Pose::default(); MAX_POSES],
            panels: Vec::new(),
            next_id: 1,
            all_hidden: false,
            zoom: None,
            mouse_hand: None,
            last_mouse: (-1., -1.),
            menu,
            menu_open: false,
            menu_tf: Mat34::identity(),
            menu_rows: Vec::new(),
            menu_layout: ui::MenuLayout::new(&[]),
            menu_hover: None,
            kb: Keyboard {
                handle: kb_handle,
                open: false,
                tf: Mat34::identity(),
                held: None,
                keys: ui::keyboard_keys(kbd::for_lcid(0x12)),
                layout: kbd::for_lcid(0x12),
                mods: ui::KbMods { shift: false, ctrl: false, alt: false },
                for_panel: None,
                ime: ime::ImeState { lcid: 0x12, native: false },
                ime_check_at: now,
            },
            settings: settings_handle,
            settings_open: false,
            settings_layout: ui::SettingsLayout::new(&[]),
            settings_hover: None,
            autostart: false,
            wrist,
            wrist_rel,
            wrist_held: None,
            wrist_key: String::new(),
            wrist_base: None,
            wrist_vis: false,
            wrist_alpha: 0.0,
            wrist_next_frame: now,
            t0: now,
            tweens: Vec::new(),
            images: std::collections::HashMap::new(),
            wrist_force_until: now,
            boot: None,
            boot_pending: cfg_boot_anim,
            battery_at: now - Duration::from_secs(3600),
            batteries: Vec::new(),
            last_input: now,
            layout_path: exe_dir().join("layout.txt"),
            layout_dirty: false,
            admin_job: None,
        };
        for h in 0..2 {
            let cursor = app.hands[h].cursor;
            app.set_image(cursor, &cursor_img);
        }
        Ok(app)
    }

    /// Show a CPU-drawn image on an overlay through our own GPU texture.
    fn set_image(&mut self, handle: Handle, c: &ui::Canvas) {
        use windows::core::Interface;
        let slot = self.images.entry(handle).or_default();
        let i = slot.next;
        slot.next ^= 1;
        let fits = matches!(&slot.tex[i], Some((_, w, h)) if *w == c.w && *h == c.h);
        if !fits {
            slot.tex[i] = match self.gpu.ui_texture(c.w, c.h) {
                Ok(t) => Some((t, c.w, c.h)),
                Err(e) => {
                    log!("ui texture {}x{}: {e}", c.w, c.h);
                    None
                }
            };
        }
        let Some((tex, _, _)) = &slot.tex[i] else { return };
        unsafe {
            self.gpu.ctx.UpdateSubresource(tex, 0, None, c.px.as_ptr() as *const _, c.w * 4, 0);
            self.gpu.ctx.Flush();
        }
        if let Err(e) = self.vr.set_texture_d3d11(handle, tex.as_raw()) {
            log!("image: {e}");
        }
    }

    fn panel_idx(&self, id: u64) -> Option<usize> {
        self.panels.iter().position(|p| p.id == id)
    }

    fn held_tf(&self, held: Option<(usize, Mat34)>, fallback: Mat34) -> Mat34 {
        match held {
            Some((h, rel)) => self.hands[h].raw.map(|r| r.mul(&rel)).unwrap_or(fallback),
            None => fallback,
        }
    }

    fn panel_tf(&self, p: &Panel) -> Mat34 {
        if p.held.is_some() {
            return self.held_tf(p.held, p.world);
        }
        self.held_tf(p.attached, p.world)
    }

    /// The hand wearing the watch (non-dominant); the other one points at it.
    fn wrist_hand(&self) -> usize {
        if self.cfg.left_handed {
            RIGHT
        } else {
            LEFT
        }
    }

    fn kb_tf(&self) -> Mat34 {
        self.held_tf(self.kb.held, self.kb.tf)
    }

    fn wrist_tf(&self) -> Option<Mat34> {
        match self.wrist_held {
            Some((h, rel)) => self.hands[h].raw.map(|r| r.mul(&rel)),
            None => Some(self.hands[self.wrist_hand()].raw?.mul(self.wrist_rel.as_ref()?)),
        }
    }

    fn wrist_size(&self) -> (f32, f32) {
        let w = self.cfg.wrist_width_m;
        (w, w * ui::WRIST_H as f32 / ui::WRIST_W as f32)
    }

    fn hmd(&self) -> Option<Mat34> {
        let p = &self.poses[0];
        p.valid.then_some(p.m)
    }

    fn engaged(&self) -> bool {
        self.menu_open
            || self.settings_open
            || self.boot.is_some()
            || !self.tweens.is_empty()
            || self.kb.open
            || self.zoom.is_some()
            || self.hands.iter().any(|h| h.target.is_some() || h.holding.is_some() || h.mouse_down.is_some())
    }

    fn haptic(&self, h: usize, amp: f32) {
        self.vr.haptic(self.act.haptic, self.hands[h].src, 0.012, 160.0, amp);
    }

    // ------------------------------------------------------------ panels

    /// Push panel + toolbar transforms to SteamVR (device-relative while held
    /// or carried, so the compositor moves them with zero lag).
    fn apply_panel_tf(&mut self, i: usize) {
        self.layout_dirty = true;
        let p = &self.panels[i];
        let off = p.tb_offset();
        self.vr.set_width(p.tb, p.tb_w());
        match p.held.or(p.attached) {
            Some((h, rel)) => {
                let dev = self.hands[h].dev;
                self.vr.set_transform_device_relative(p.handle, dev, &rel);
                self.vr.set_transform_device_relative(p.tb, dev, &rel.mul(&off));
            }
            None => {
                self.vr.set_transform_absolute(p.handle, &p.world);
                self.vr.set_transform_absolute(p.tb, &p.world.mul(&off));
            }
        }
    }

    fn redraw_toolbar(&mut self, i: usize) {
        let now = Instant::now();
        let fps = self.panels[i].fps(&self.cfg, now);
        let kb_open = self.kb.open && self.kb.for_panel == Some(self.panels[i].id);
        let _ = fps;
        let pinned = self.panels[i].attached.is_some();
        let (canvas, buttons) = ui::toolbar(&ui::ToolbarState { live: self.panels[i].live, kb_open, pinned });
        let tb = self.panels[i].tb;
        self.set_image(tb, &canvas);
        let p = &mut self.panels[i];
        p.buttons = buttons;
        p.tb_drawn = Some((p.live, kb_open, pinned));
    }

    fn spawn(&mut self, src: Source, near: Option<Mat34>) {
        if let Some(i) = self.panels.iter().position(|p| p.src.same(&src)) {
            // Already open: bring it to where the user is looking.
            if let Some(tf) = self.spawn_tf(near) {
                self.panels[i].world = tf;
                self.apply_panel_tf(i);
            }
            return;
        }
        let world = self.spawn_tf(near).unwrap_or_else(|| Mat34::pitch_translate(0.0, [0.0, 1.4, -self.cfg.spawn_distance_m]));
        self.spawn_at(src, world, None);
    }

    /// Create a panel at `world`; returns its index.
    fn spawn_at(&mut self, src: Source, world: Mat34, width: Option<f32>) -> Option<usize> {
        let cap = match Capture::new(&self.gpu, &src) {
            Ok(c) => c,
            Err(e) => {
                log!("cannot capture {}: {e}", src.label());
                return None;
            }
        };
        let id = self.next_id;
        self.next_id += 1;
        let (handle, tb) = match (
            self.vr.create_overlay(&format!("ufo.panel.{id}"), &src.label()),
            self.vr.create_overlay(&format!("ufo.toolbar.{id}"), "UFO toolbar"),
        ) {
            (Ok(h), Ok(t)) => (h, t),
            (a, b) => {
                log!("overlay create failed: {:?} {:?}", a.err(), b.err());
                return None;
            }
        };
        let width_m = width.unwrap_or(match src {
            Source::Monitor { .. } => 1.2,
            Source::Window { .. } => (cap.w as f32 / 1500.0).clamp(0.35, 1.4),
        });
        self.vr.set_width(handle, width_m);
        self.vr.set_alpha(handle, self.cfg.panel_alpha);
        self.vr.set_flag(handle, vr::flag::IGNORE_TEXTURE_ALPHA, true);
        self.vr.set_sort_order(tb, 5);
        log!("panel {id}: {} {}x{}", src.label(), cap.w, cap.h);
        let now = Instant::now();
        self.panels.push(Panel {
            id,
            handle,
            src,
            cap,
            width_m,
            world,
            held: None,
            attached: None,
            live: false,
            boost_until: now + Duration::from_secs(2),
            last_submit: now - Duration::from_secs(1),
            dirty: false,
            tb,
            tb_on: false,
            tb_until: now + Duration::from_secs(3),
            buttons: Vec::new(),
            tb_drawn: None,
        });
        let i = self.panels.len() - 1;
        self.apply_panel_tf(i);
        self.redraw_toolbar(i);
        if !self.all_hidden {
            self.vr.show(handle);
            self.pop(handle, width_m, self.cfg.panel_alpha);
        }
        Some(i)
    }

    /// Fan every free-standing panel out in an arc in front of the user.
    fn gather(&mut self) {
        let Some(head) = self.hmd() else { return };
        let eye = head.pos();
        let mut f = head.forward();
        f.1 = 0.0;
        let f = f.norm();
        let right = f.cross(V3::UP).norm();
        let r = self.cfg.spawn_distance_m.max(0.6);
        let ids: Vec<usize> =
            (0..self.panels.len()).filter(|&i| self.panels[i].attached.is_none() && self.panels[i].held.is_none()).collect();
        let gap = 0.08;
        let total: f32 = ids.iter().map(|&i| self.panels[i].width_m).sum::<f32>() + gap * ids.len().saturating_sub(1) as f32;
        let mut x = -total / 2.0;
        for i in ids {
            let w = self.panels[i].width_m;
            let ang = (x + w / 2.0) / r; // arc length → angle
            x += w + gap;
            let dir = f * ang.cos() + right * ang.sin();
            let pos = eye + dir * r - V3(0., 0.08, 0.);
            self.panels[i].world = Mat34::facing(pos, eye);
            self.apply_panel_tf(i);
            let (h, wm, a) = (self.panels[i].handle, w, self.cfg.panel_alpha);
            if !self.all_hidden {
                self.pop(h, wm, a);
            }
        }
    }

    // ------------------------------------------------------------ layout

    fn save_layout(&self) {
        let saved: Vec<layout::Saved> = self
            .panels
            .iter()
            .map(|p| {
                let src = match &p.src {
                    Source::Monitor { rect, .. } => layout::SavedSrc::Monitor { rect: [rect.left, rect.top, rect.right, rect.bottom] },
                    Source::Window { hwnd, title } => {
                        let (exe, class) = sources::window_identity(*hwnd);
                        layout::SavedSrc::Window { exe, class, title: title.clone() }
                    }
                };
                // While grabbed, store where it currently is.
                let world = if p.held.is_some() { self.panel_tf(p) } else { p.world };
                layout::Saved {
                    src,
                    width: p.width_m,
                    live: p.live,
                    world: mat_to_12(&world),
                    attached: p.attached.map(|(h, m)| (h, mat_to_12(&m))),
                }
            })
            .collect();
        layout::save(&self.layout_path, &saved);
    }

    /// Reopen last session's panels. Windows are matched by exe + class
    /// (title preferred); ones that aren't open right now are skipped.
    fn restore_layout(&mut self) {
        let saved = layout::load(&self.layout_path);
        if saved.is_empty() {
            return;
        }
        let mons = sources::monitors();
        let wins: Vec<(Source, (String, String))> = sources::windows()
            .into_iter()
            .map(|w| {
                let id = match &w {
                    Source::Window { hwnd, .. } => sources::window_identity(*hwnd),
                    _ => Default::default(),
                };
                (w, id)
            })
            .collect();
        let mut used: Vec<usize> = Vec::new();
        for sv in saved {
            let src = match &sv.src {
                layout::SavedSrc::Monitor { rect } => {
                    // Exact desktop position first; if the monitors were
                    // rearranged, fall back to the one with the same resolution.
                    let size = |r: &[i32; 4]| (r[2] - r[0], r[3] - r[1]);
                    let as_arr = |m: &Source| match m {
                        Source::Monitor { rect: r, .. } => Some([r.left, r.top, r.right, r.bottom]),
                        _ => None,
                    };
                    mons.iter()
                        .find(|m| as_arr(m) == Some(*rect))
                        .or_else(|| mons.iter().find(|m| as_arr(m).map(|r| size(&r)) == Some(size(rect))))
                        .cloned()
                }
                layout::SavedSrc::Window { exe, class, title } => {
                    let cands: Vec<usize> =
                        (0..wins.len()).filter(|&k| !used.contains(&k) && wins[k].1 .0 == *exe && wins[k].1 .1 == *class).collect();
                    let pick = cands.iter().copied().find(|&k| wins[k].0.label() == *title).or(cands.first().copied());
                    pick.map(|k| {
                        used.push(k);
                        wins[k].0.clone()
                    })
                }
            };
            let Some(src) = src else {
                log!("layout: {:?} is not open, skipped", sv.src);
                continue;
            };
            if let Some(i) = self.spawn_at(src, mat_from_12(&sv.world), Some(sv.width)) {
                self.panels[i].live = sv.live;
                self.panels[i].attached = sv.attached.map(|(h, m)| (h, mat_from_12(&m)));
                self.apply_panel_tf(i);
            }
        }
        log!("layout: restored {} panel(s)", self.panels.len());
    }

    /// In front of the user, at spawn distance, along `near` (e.g. the menu) if given.
    fn spawn_tf(&self, near: Option<Mat34>) -> Option<Mat34> {
        let head = self.hmd()?;
        let eye = head.pos();
        let dir = match near {
            Some(n) => (n.pos() - eye).norm(),
            None => {
                let mut f = head.forward();
                f.1 = 0.0;
                f.norm()
            }
        };
        let pos = eye + dir * self.cfg.spawn_distance_m;
        Some(Mat34::facing(pos, eye))
    }

    fn close_panel(&mut self, id: u64) {
        let Some(i) = self.panel_idx(id) else { return };
        let p = self.panels.remove(i);
        self.vr.destroy_overlay(p.handle);
        self.vr.destroy_overlay(p.tb);
        self.images.remove(&p.tb);
        for h in &mut self.hands {
            if h.holding == Some(Held::Panel(id)) {
                h.holding = None;
            }
            if h.target.and_then(|t| t.panel_id()) == Some(id) {
                h.target = None;
            }
        }
        if self.zoom.as_ref().is_some_and(|z| z.panel == id) {
            self.zoom = None;
        }
        if self.kb.for_panel == Some(id) {
            self.kb.for_panel = None;
        }
        self.layout_dirty = true;
        log!("panel {id} closed");
    }

    fn set_all_hidden(&mut self, hidden: bool) {
        self.all_hidden = hidden;
        for p in &mut self.panels {
            if hidden {
                self.vr.hide(p.handle);
                self.vr.hide(p.tb);
                p.tb_on = false;
                p.cap.stop();
            } else {
                self.vr.show(p.handle);
                p.dirty = true;
            }
        }
    }

    /// Show a panel's toolbar only while it is being pointed at / held.
    fn update_toolbars(&mut self) {
        let now = Instant::now();
        for i in 0..self.panels.len() {
            let p = &self.panels[i];
            let kb_here = self.kb.open && self.kb.for_panel == Some(p.id);
            let want = !self.all_hidden && (now < p.tb_until || p.held.is_some() || kb_here);
            if want && (!p.tb_on || p.tb_drawn != Some((p.live, kb_here, p.attached.is_some()))) {
                self.redraw_toolbar(i);
            }
            let p = &self.panels[i];
            if want != p.tb_on {
                let (tb, w) = (p.tb, p.tb_w());
                if want {
                    self.vr.show(tb);
                    self.pop(tb, w, 1.0);
                } else {
                    self.vr.hide(tb);
                }
                self.panels[i].tb_on = want;
            }
        }
    }

    // -------------------------------------------------------------- menu

    fn open_menu(&mut self) {
        self.open_menu_at(None);
    }

    /// Open the menu in front of the user, or exactly at `at` (coming back from settings).
    fn open_menu_at(&mut self, at: Option<Mat34>) {
        let Some(head) = self.hmd() else { return };
        let mut rows = sources::monitors();
        rows.extend(sources::windows().into_iter().take(14));
        self.menu_rows = rows;
        self.menu_tf = at.unwrap_or_else(|| {
            let mut f = head.forward();
            f.1 = 0.0;
            let pos = head.pos() + f.norm() * 0.6 - V3(0., 0.12, 0.);
            Mat34::facing(pos, head.pos())
        });
        self.vr.set_transform_absolute(self.menu, &self.menu_tf);
        self.menu_hover = None;
        self.redraw_menu();
        self.vr.show(self.menu);
        self.pop(self.menu, MENU_W_M, 1.0);
        self.menu_open = true;
    }

    fn close_menu(&mut self) {
        self.vr.hide(self.menu);
        self.menu_open = false;
        for h in &mut self.hands {
            if matches!(h.target, Some(Target::Menu(..))) {
                h.target = None;
            }
        }
    }

    fn redraw_menu(&mut self) {
        let rows: Vec<ui::MenuItem> = self
            .menu_rows
            .iter()
            .map(|s| (s.label(), matches!(s, Source::Monitor { .. }), self.panels.iter().any(|p| p.src.same(s))))
            .collect();
        let (c, lay) = ui::menu(&rows, self.menu_hover, self.all_hidden);
        self.menu_layout = lay;
        self.set_image(self.menu, &c);
    }

    fn menu_h_m(&self) -> f32 {
        MENU_W_M * self.menu_layout.h as f32 / ui::MENU_W as f32
    }

    // ---------------------------------------------------------- settings

    fn open_settings(&mut self) {
        self.autostart = self.vr.auto_launch(APP_KEY);
        let tf = self.menu_tf;
        self.close_menu();
        self.vr.set_transform_absolute(self.settings, &tf);
        self.settings_hover = None;
        self.redraw_settings();
        self.vr.show(self.settings);
        self.pop(self.settings, SET_W_M, 1.0);
        self.settings_open = true;
    }

    fn close_settings(&mut self) {
        self.vr.hide(self.settings);
        self.settings_open = false;
        for h in &mut self.hands {
            if matches!(h.target, Some(Target::Settings(..))) {
                h.target = None;
            }
        }
    }

    fn settings_rows(&self) -> Vec<ui::RowView> {
        settings::items()
            .into_iter()
            .map(|it| {
                let ctrl = match it.kind {
                    settings::Kind::Num { min, max, step } => {
                        let max = if it.id == settings::Id::Language { settings::language_max() } else { max };
                        let v = settings::num(&self.cfg, it.id);
                        ui::Ctrl::Num(settings::format(it.id, v), v > min + step * 0.01, v < max - step * 0.01)
                    }
                    settings::Kind::Toggle => ui::Ctrl::Toggle(match it.id {
                        settings::Id::Wrist24h => self.cfg.wrist_24h,
                        settings::Id::BootAnim => self.cfg.boot_anim,
                        settings::Id::Autostart => self.autostart,
                        settings::Id::BlockInput => self.cfg.block_input,
                        settings::Id::LeftHanded => self.cfg.left_handed,
                        settings::Id::Admin => self.cfg.admin,
                        _ => false,
                    }),
                    settings::Kind::Action(name) => ui::Ctrl::Action(i18n::tr(name)),
                };
                ui::RowView { section: i18n::tr(it.section), label: i18n::tr(it.label), ctrl }
            })
            .collect()
    }

    fn redraw_settings(&mut self) {
        let rows = self.settings_rows();
        let (c, lay) = ui::settings(&rows, self.settings_hover);
        self.settings_layout = lay;
        self.set_image(self.settings, &c);
    }

    fn settings_h_m(&self) -> f32 {
        SET_W_M * self.settings_layout.h as f32 / ui::SET_W as f32
    }

    /// Apply a settings click; changes take effect immediately and are saved.
    fn settings_click(&mut self, hit: ui::SetHit) {
        use settings::Id;
        let items = settings::items();
        let id_of = |i: usize| items.get(i).map(|it| it.id);
        match hit {
            ui::SetHit::Back => {
                let tf = self.menu_tf;
                self.close_settings();
                self.open_menu_at(Some(tf));
                return;
            }
            ui::SetHit::Minus(i) | ui::SetHit::Plus(i) => {
                let Some(id) = id_of(i) else { return };
                settings::step(&mut self.cfg, id, if matches!(hit, ui::SetHit::Plus(_)) { 1 } else { -1 });
            }
            ui::SetHit::Toggle(i) => match id_of(i) {
                Some(Id::Wrist24h) => {
                    self.cfg.wrist_24h = !self.cfg.wrist_24h;
                    self.wrist_key.clear();
                }
                Some(Id::BootAnim) => self.cfg.boot_anim = !self.cfg.boot_anim,
                Some(Id::BlockInput) => self.cfg.block_input = !self.cfg.block_input,
                Some(Id::LeftHanded) => {
                    self.cfg.left_handed = !self.cfg.left_handed;
                    // The watch moves hands: forget the old spot, re-derive it.
                    self.cfg.wrist_pose = None;
                    self.wrist_rel = None;
                    self.wrist_held = None;
                    self.wrist_key.clear();
                    self.wrist_force_until = Instant::now() + Duration::from_secs(3);
                }
                Some(Id::Admin) => self.toggle_admin(),
                Some(Id::Autostart) => {
                    let want = !self.autostart;
                    let r = if want {
                        write_manifest().and_then(|m| self.vr.register_manifest(&m.to_string_lossy(), APP_KEY))
                    } else {
                        self.vr.set_auto_launch(APP_KEY, false)
                    };
                    if let Err(e) = r {
                        log!("autostart: {e}");
                    }
                    self.autostart = self.vr.auto_launch(APP_KEY);
                    log!("autostart now {}", self.autostart);
                }
                _ => {}
            },
            ui::SetHit::Action(i) => {
                if id_of(i) == Some(Id::WristReset) {
                    self.cfg.wrist_pose = None;
                    self.wrist_rel = None; // recomputed from the aim pose next tick
                    self.wrist_key.clear();
                    self.wrist_force_until = Instant::now() + Duration::from_secs(2);
                }
            }
            ui::SetHit::Row(_) => return,
        }
        self.cfg.sanitize();
        self.cfg.save(&self.ini);
        let before = i18n::code();
        i18n::set(&self.cfg.language);
        if i18n::code() != before {
            // Re-render everything that has words on it.
            for p in &mut self.panels {
                p.tb_drawn = None;
            }
            if self.kb.open {
                self.redraw_keyboard();
            }
        }
        // Live-apply what needs a nudge.
        for p in &self.panels {
            self.vr.set_alpha(p.handle, self.cfg.panel_alpha);
        }
        if let Some(wh) = self.wrist {
            self.vr.set_width(wh, self.cfg.wrist_width_m);
        }
        self.redraw_settings();
    }

    // ------------------------------------------------------------- admin

    fn toggle_admin(&mut self) {
        if self.cfg.admin {
            self.cfg.admin = false;
            if let Err(e) = elevate::delete_task() {
                log!("admin: removing task failed: {e}");
            }
            log!("admin option off (takes effect next start)");
            return;
        }
        if self.admin_job.is_some() {
            return;
        }
        // The UAC prompt appears on the PC screen; wait for it off the VR thread.
        let (tx, rx) = std::sync::mpsc::channel();
        let exe = std::env::current_exe().unwrap_or_default();
        std::thread::spawn(move || {
            let _ = tx.send(elevate::create_task(&exe));
        });
        self.admin_job = Some(rx);
        log!("admin: waiting for UAC approval on the PC");
    }

    fn admin_task_done(&mut self, r: Result<(), String>) {
        match r {
            Ok(()) => {
                self.cfg.admin = true;
                self.cfg.save(&self.ini);
                if self.settings_open {
                    self.redraw_settings();
                }
                if !elevate::is_elevated() {
                    log!("admin: task created, relaunching elevated");
                    self.save_layout();
                    release_single_instance();
                    if elevate::run_task() {
                        std::process::exit(0);
                    }
                    log!("admin: could not start the task");
                }
            }
            Err(e) => log!("admin: {e}"),
        }
    }

    // ---------------------------------------------------------- keyboard

    fn open_keyboard(&mut self, for_panel: Option<u64>) {
        let Some(head) = self.hmd() else { return };
        let mut f = head.forward();
        f.1 = 0.0;
        let pos = head.pos() + f.norm() * 0.45 - V3(0., 0.32, 0.);
        // Upright toward the eye, then tilted back like a desk keyboard.
        self.kb.tf = Mat34::facing(pos, head.pos()).mul(&Mat34::pitch_translate(-35.0, [0.0, 0.0, 0.0]));
        self.kb.held = None;
        self.kb.for_panel = for_panel;
        self.vr.set_transform_absolute(self.kb.handle, &self.kb.tf);
        self.kb.ime = ime::state();
        self.kb.layout = kbd::for_lcid(self.kb.ime.lcid);
        self.kb.keys = ui::keyboard_keys(self.kb.layout);
        self.kb.ime_check_at = Instant::now() + Duration::from_millis(250);
        self.redraw_keyboard();
        self.vr.show(self.kb.handle);
        self.pop(self.kb.handle, KB_W_M, 1.0);
        self.kb.open = true;
    }

    fn close_keyboard(&mut self) {
        self.vr.hide(self.kb.handle);
        self.kb.open = false;
        self.kb.held = None;
        for h in &mut self.hands {
            if h.holding == Some(Held::Keyboard) {
                h.holding = None;
            }
        }
    }

    fn redraw_keyboard(&mut self) {
        let c = ui::keyboard(&self.kb.keys, self.kb.layout, &self.kb.mods, self.kb.ime);
        self.set_image(self.kb.handle, &c);
    }

    fn kb_h_m(&self) -> f32 {
        KB_W_M * ui::KB_H as f32 / ui::KB_W as f32
    }

    fn press_key(&mut self, u: f32, v: f32) -> bool {
        let (px, py) = (u * ui::KB_W as f32, v * ui::KB_H as f32);
        let Some(kind) = ui::key_at(&self.kb.keys, px, py).map(|k| k.def.kind) else { return false };
        let m = &mut self.kb.mods;
        match kind {
            ui::KeyKind::Vk(vk, ext) => {
                input::key_press(vk, ext, m.shift, m.ctrl, m.alt);
                if m.shift || m.ctrl || m.alt {
                    *m = ui::KbMods { shift: false, ctrl: false, alt: false };
                    self.redraw_keyboard();
                }
            }
            ui::KeyKind::Shift => {
                m.shift = !m.shift;
                self.redraw_keyboard();
            }
            ui::KeyKind::Ctrl => {
                m.ctrl = !m.ctrl;
                self.redraw_keyboard();
            }
            ui::KeyKind::Alt => {
                m.alt = !m.alt;
                self.redraw_keyboard();
            }
            ui::KeyKind::Ime => {
                if !ime::toggle() && self.kb.layout.ime_vk != 0 {
                    // No IME window to talk to: send the layout's toggle key instead.
                    input::key_press(self.kb.layout.ime_vk, false, false, false, false);
                }
                // Re-read the mode right away so the labels flip.
                self.kb.ime_check_at = Instant::now() + Duration::from_millis(60);
            }
            ui::KeyKind::Close => self.close_keyboard(),
        }
        true
    }

    /// Follow the focused window's IME: layout (ko/ja/en) and 한/영 · あ/A.
    fn poll_ime(&mut self) {
        if !self.kb.open || Instant::now() < self.kb.ime_check_at {
            return;
        }
        self.kb.ime_check_at = Instant::now() + Duration::from_millis(250);
        let st = ime::state();
        if st != self.kb.ime {
            let layout = kbd::for_lcid(st.lcid);
            if layout.code != self.kb.layout.code {
                self.kb.layout = layout;
                self.kb.keys = ui::keyboard_keys(layout);
            }
            self.kb.ime = st;
            self.redraw_keyboard();
        }
    }

    // ------------------------------------------------------------- input

    fn target_tf(&self, t: &Target) -> Option<Mat34> {
        match t {
            Target::Panel(id, ..) => self.panel_idx(*id).map(|i| self.panel_tf(&self.panels[i])),
            Target::Toolbar(id, ..) => self.panel_idx(*id).map(|i| {
                let p = &self.panels[i];
                self.panel_tf(p).mul(&p.tb_offset())
            }),
            Target::Menu(..) => Some(self.menu_tf),
            Target::Keyboard(..) => Some(self.kb_tf()),
            Target::Settings(..) => Some(self.menu_tf),
            Target::Wrist => self.wrist_tf(),
        }
    }

    fn raycast(&self, h: usize) -> Option<(Target, f32)> {
        let aim = self.hands[h].aim?;
        let (o, d) = (aim.pos(), aim.forward());
        let mut best: Option<(Target, f32)> = None;
        let mut consider = |t: Target, hit: Option<(f32, f32, f32)>| {
            if let Some((dist, u, v)) = hit {
                if best.as_ref().is_none_or(|b| dist < b.1) {
                    let t = match t {
                        Target::Panel(id, ..) => Target::Panel(id, u, v),
                        Target::Toolbar(id, ..) => Target::Toolbar(id, u, v),
                        Target::Menu(..) => Target::Menu(u, v),
                        Target::Keyboard(..) => Target::Keyboard(u, v),
                        Target::Settings(..) => Target::Settings(u, v),
                        Target::Wrist => Target::Wrist,
                    };
                    best = Some((t, dist));
                }
            }
        };
        if !self.all_hidden {
            for p in &self.panels {
                let tf = self.panel_tf(p);
                consider(Target::Panel(p.id, 0., 0.), ray_quad(o, d, &tf, p.width_m, p.height_m()));
                if p.tb_on {
                    consider(Target::Toolbar(p.id, 0., 0.), ray_quad(o, d, &tf.mul(&p.tb_offset()), p.tb_w(), p.tb_h()));
                }
            }
        }
        if self.menu_open {
            consider(Target::Menu(0., 0.), ray_quad(o, d, &self.menu_tf, MENU_W_M, self.menu_h_m()));
        }
        if self.kb.open {
            consider(Target::Keyboard(0., 0.), ray_quad(o, d, &self.kb_tf(), KB_W_M, self.kb_h_m()));
        }
        if self.settings_open {
            consider(Target::Settings(0., 0.), ray_quad(o, d, &self.menu_tf, SET_W_M, self.settings_h_m()));
        }
        // The watch rides on one hand, so only the other hand can point at it.
        if h != self.wrist_hand() && self.wrist.is_some() && (self.wrist_vis || self.wrist_held.is_some()) {
            if let Some(tf) = self.wrist_tf() {
                let (w, wh) = self.wrist_size();
                consider(Target::Wrist, ray_quad(o, d, &tf, w, wh));
            }
        }
        best
    }

    fn input_tick(&mut self, dt: f32) {
        // A hand that is on UFOverlay keeps its buttons away from the game.
        let pri = |h: &Hand| {
            if self.cfg.block_input && (h.target.is_some() || h.holding.is_some() || h.mouse_down.is_some()) {
                Vr::PRIORITY_OVER_GAME
            } else {
                0
            }
        };
        let sets = [(self.hands[0].src, pri(&self.hands[0])), (self.hands[1].src, pri(&self.hands[1]))];
        let e = self.vr.update_actions(self.act.set, &sets);
        if e != 0 && cfg!(debug_assertions) {
            log!("[dbg] UpdateActionState error {e} sets={sets:?}");
        }
        if cfg!(debug_assertions) && sets.iter().any(|s| s.1 != 0) {
            log!("[dbg] blocking game input: {:?}", sets.iter().map(|s| s.1 != 0).collect::<Vec<_>>());
        }
        self.vr.device_poses(&mut self.poses);
        for h in 0..2 {
            let src = self.hands[h].src;
            let dev = self.hands[h].dev;
            let raw = (dev < MAX_POSES as u32 && self.poses[dev as usize].valid).then(|| self.poses[dev as usize].m);
            let hand = &mut self.hands[h];
            hand.raw = raw;
            let aim = self.vr.pose(self.act.aim, src);
            hand.aim = match (aim, hand.aim) {
                (Some(cur), Some(prev)) if self.cfg.smoothing > 0 => Some(smooth_pose(&prev, &cur, dt, self.cfg.smoothing)),
                (cur, _) => cur,
            };
            hand.trig = self.vr.digital(self.act.click, src);
            hand.grip = self.vr.digital(self.act.grab, src);
            hand.rclick = self.vr.digital(self.act.rclick, src);
            hand.stick = self.vr.analog(self.act.stick, src);
        }
        for h in 0..2 {
            self.hand_tick(h, dt);
        }
        self.zoom_tick();
        self.poll_ime();
        self.update_toolbars();
        self.update_wrist_visibility();
    }

    fn grab(&mut self, h: usize, t: Target) {
        let Some(raw) = self.hands[h].raw else { return };
        let inv = raw.inv_rigid();
        let dev = self.hands[h].dev;
        match t {
            Target::Panel(id, ..) | Target::Toolbar(id, ..) => {
                let other = 1 - h;
                let Some(i) = self.panel_idx(id) else { return };
                if self.hands[other].holding == Some(Held::Panel(id)) {
                    // Second hand on a held panel → two-hand zoom.
                    if let (Some(a), Some(b)) = (self.hands[0].raw, self.hands[1].raw) {
                        let d0 = (a.pos() - b.pos()).len().max(0.05);
                        self.zoom = Some(Zoom { panel: id, hand: h, d0, w0: self.panels[i].width_m });
                    }
                } else {
                    let rel = inv.mul(&self.panel_tf(&self.panels[i]));
                    self.panels[i].held = Some((h, rel));
                    self.hands[h].holding = Some(Held::Panel(id));
                    self.apply_panel_tf(i);
                }
            }
            Target::Keyboard(..) => {
                let rel = inv.mul(&self.kb_tf());
                self.kb.held = Some((h, rel));
                self.hands[h].holding = Some(Held::Keyboard);
                self.vr.set_transform_device_relative(self.kb.handle, dev, &rel);
            }
            Target::Wrist => {
                let (Some(wh), Some(tf)) = (self.wrist, self.wrist_tf()) else { return };
                let rel = inv.mul(&tf);
                self.wrist_held = Some((h, rel));
                self.hands[h].holding = Some(Held::Wrist);
                self.vr.set_transform_device_relative(wh, dev, &rel);
            }
            Target::Menu(..) | Target::Settings(..) => return,
        }
        self.haptic(h, 0.4);
    }

    fn release(&mut self, h: usize) {
        if self.zoom.as_ref().is_some_and(|z| z.hand == h) {
            self.zoom = None;
        }
        let Some(held) = self.hands[h].holding.take() else { return };
        match held {
            Held::Panel(id) => {
                if self.zoom.as_ref().is_some_and(|z| z.panel == id) {
                    self.zoom = None;
                }
                if let Some(i) = self.panel_idx(id) {
                    let tf = self.panel_tf(&self.panels[i]);
                    self.panels[i].world = tf;
                    self.panels[i].held = None;
                    if let Some((ah, _)) = self.panels[i].attached {
                        if let Some(raw) = self.hands[ah].raw {
                            self.panels[i].attached = Some((ah, raw.inv_rigid().mul(&tf)));
                        }
                    }
                    self.apply_panel_tf(i);
                }
            }
            Held::Keyboard => {
                self.kb.tf = self.kb_tf();
                self.kb.held = None;
                self.vr.set_transform_absolute(self.kb.handle, &self.kb.tf);
            }
            Held::Wrist => {
                let world = self.wrist_tf();
                self.wrist_held = None;
                let wh_hand = self.wrist_hand();
                if let (Some(world), Some(base), Some(wh)) = (world, self.hands[wh_hand].raw, self.wrist) {
                    let rel = base.inv_rigid().mul(&world);
                    self.wrist_rel = Some(rel);
                    self.vr.set_transform_device_relative(wh, self.hands[wh_hand].dev, &rel);
                    self.cfg.wrist_pose = Some(mat_to_12(&rel));
                    self.cfg.save(&self.ini);
                    log!("wrist placement saved");
                }
            }
        }
    }

    fn hand_tick(&mut self, h: usize, dt: f32) {
        let hit = self.raycast(h);
        let prev = self.hands[h].target;
        let target = hit.map(|t| t.0);
        self.hands[h].target = target;
        self.hands[h].dist = hit.map_or(0., |t| t.1);
        if let Some(t) = target {
            if prev.is_none_or(|p| !p.same_object(&t)) {
                self.haptic(h, 0.12);
            }
        }
        let (grip_down, grip_up) = edge(self.hands[h].grip);
        let (trig_down, trig_up) = edge(self.hands[h].trig);
        let (r_down, r_up) = edge(self.hands[h].rclick);
        let now = Instant::now();
        if let Some(id) = target.and_then(|t| t.panel_id()) {
            if let Some(i) = self.panel_idx(id) {
                self.panels[i].boost_until = now + Duration::from_secs_f32(self.cfg.boost_secs);
                self.panels[i].tb_until = now + TOOLBAR_LINGER;
            }
        }

        if grip_down {
            if let Some(t) = target {
                self.grab(h, t);
            }
        }
        if grip_up {
            self.release(h);
        }

        // ---- holding a panel: stick pushes it away / pulls it closer
        if let Some(held) = self.hands[h].holding {
            if let Held::Panel(id) = held {
                let sy = self.hands[h].stick.1;
                if let Some(i) = self.panel_idx(id) {
                    self.panels[i].boost_until = now + Duration::from_secs_f32(self.cfg.boost_secs);
                    if self.zoom.is_none() && sy.abs() > 0.15 {
                        if let Some((hh, mut rel)) = self.panels[i].held {
                            let t = rel.pos();
                            let dist = t.len();
                            if dist > 1e-3 {
                                let nd = (dist + sy * self.cfg.push_speed * dt).clamp(0.12, 15.0);
                                rel.set_pos(t * (nd / dist));
                                self.panels[i].held = Some((hh, rel));
                                self.apply_panel_tf(i);
                            }
                        }
                    }
                }
            }
            self.update_cursor(h);
            return;
        }

        // ---- pointing
        match target {
            Some(Target::Panel(id, u, v)) => {
                let Some(i) = self.panel_idx(id) else { return };
                let (w, ch) = (self.panels[i].cap.w as f32, self.panels[i].cap.h as f32);
                let (px, py) = (u * w, v * ch);
                if let Some(r) = self.panels[i].src.screen_rect() {
                    let sx = r.left as f32 + px * (r.right - r.left) as f32 / w;
                    let sy = r.top as f32 + py * (r.bottom - r.top) as f32 / ch;
                    let other_busy = self.hands[1 - h].mouse_down.is_some();
                    if self.mouse_hand != Some(h) && (!other_busy || trig_down) {
                        self.mouse_hand = Some(h);
                    }
                    if self.mouse_hand == Some(h) {
                        let frozen = now < self.hands[h].freeze_until;
                        if !frozen && ((sx - self.last_mouse.0).abs() >= 0.5 || (sy - self.last_mouse.1).abs() >= 0.5) {
                            input::move_to(sx, sy);
                            self.last_mouse = (sx, sy);
                        }
                        if trig_down || r_down {
                            if let Source::Window { hwnd, .. } = self.panels[i].src {
                                input::focus_window(hwnd);
                            }
                            let right = r_down && !trig_down;
                            input::button(sx, sy, right, true);
                            self.last_mouse = (sx, sy);
                            self.hands[h].mouse_down = Some(right);
                            if self.cfg.smoothing > 0 {
                                self.hands[h].freeze_until = now + Duration::from_millis(if self.cfg.smoothing >= 2 { 220 } else { 140 });
                            }
                            self.haptic(h, 0.3);
                        }
                        let stick_y = self.hands[h].stick.1;
                        if stick_y.abs() > 0.2 {
                            // ~10 notches/s at full deflection
                            self.hands[h].scroll_acc += stick_y * 10.0 * 120.0 * self.cfg.scroll_speed * dt;
                            let n = self.hands[h].scroll_acc.trunc() as i32;
                            self.hands[h].scroll_acc -= n as f32;
                            input::wheel(n);
                        }
                    }
                }
            }
            Some(Target::Toolbar(id, u, _)) if trig_down => {
                let Some(i) = self.panel_idx(id) else { return };
                let px = (u * ui::TB_W as f32) as u32;
                let hitb = self.panels[i].buttons.iter().find(|b| px >= b.x0 && px < b.x1).map(|b| b.action);
                self.haptic(h, 0.3);
                match hitb {
                    Some(ui::Action::Live) => {
                        self.panels[i].live = !self.panels[i].live;
                        self.layout_dirty = true;
                        self.redraw_toolbar(i);
                    }
                    Some(ui::Action::Pin) => {
                        let tf = self.panel_tf(&self.panels[i]);
                        if self.panels[i].attached.take().is_some() {
                            self.panels[i].world = tf;
                        } else if let Some(raw) = self.hands[1 - h].raw {
                            // Ride on the *other* hand, exactly where it is now.
                            self.panels[i].attached = Some((1 - h, raw.inv_rigid().mul(&tf)));
                        }
                        self.apply_panel_tf(i);
                        self.redraw_toolbar(i);
                    }
                    Some(ui::Action::Keyboard) => {
                        if self.kb.open && self.kb.for_panel == Some(id) {
                            self.close_keyboard();
                        } else {
                            if let Source::Window { hwnd, .. } = self.panels[i].src {
                                input::focus_window(hwnd);
                            }
                            self.open_keyboard(Some(id));
                        }
                        self.redraw_toolbar(i);
                    }
                    Some(ui::Action::Close) => {
                        self.close_panel(id);
                        self.update_cursor(h);
                        return;
                    }
                    None => {}
                }
            }
            Some(Target::Keyboard(u, v)) if trig_down => {
                if self.press_key(u, v) {
                    self.haptic(h, 0.25);
                }
            }
            Some(Target::Menu(u, v)) => {
                let (x, y) = ((u * ui::MENU_W as f32) as u32, (v * self.menu_layout.h as f32) as u32);
                let hover = self.menu_layout.hit(x, y);
                if hover != self.menu_hover {
                    self.menu_hover = hover;
                    self.redraw_menu();
                }
                if trig_down {
                    self.haptic(h, 0.3);
                    match hover {
                        Some(ui::MenuHit::Source(k)) => {
                            if let Some(src) = self.menu_rows.get(k).cloned() {
                                let near = Some(self.menu_tf);
                                self.close_menu();
                                if self.all_hidden {
                                    self.set_all_hidden(false);
                                }
                                self.spawn(src, near);
                            }
                        }
                        Some(ui::MenuHit::Refresh) => {
                            self.close_menu();
                            self.open_menu();
                        }
                        Some(ui::MenuHit::ToggleAll) => {
                            self.set_all_hidden(!self.all_hidden);
                            self.redraw_menu();
                        }
                        Some(ui::MenuHit::Gather) => {
                            if self.all_hidden {
                                self.set_all_hidden(false);
                            }
                            self.gather();
                        }
                        Some(ui::MenuHit::Settings) => self.open_settings(),
                        None => {}
                    }
                }
            }
            Some(Target::Settings(u, v)) => {
                let (x, y) = ((u * ui::SET_W as f32) as u32, (v * self.settings_layout.h as f32) as u32);
                let hover = self.settings_layout.hit(x, y);
                if hover != self.settings_hover {
                    self.settings_hover = hover;
                    self.redraw_settings();
                }
                if trig_down {
                    if let Some(hit) = hover {
                        if !matches!(hit, ui::SetHit::Row(_)) {
                            self.haptic(h, 0.3);
                        }
                        self.settings_click(hit);
                    }
                }
            }
            Some(Target::Wrist) if trig_down => {
                self.haptic(h, 0.3);
                if self.settings_open {
                    self.close_settings();
                } else if self.menu_open {
                    self.close_menu();
                } else {
                    self.open_menu();
                }
            }
            _ => {}
        }
        if self.menu_open && self.menu_hover.is_some() && !self.hands.iter().any(|x| matches!(x.target, Some(Target::Menu(..)))) {
            self.menu_hover = None;
            self.redraw_menu();
        }
        if self.settings_open && self.settings_hover.is_some() && !self.hands.iter().any(|x| matches!(x.target, Some(Target::Settings(..))))
        {
            self.settings_hover = None;
            self.redraw_settings();
        }

        // ---- releases land wherever the cursor is now
        if let Some(right) = self.hands[h].mouse_down {
            if (right && r_up) || (!right && trig_up) {
                input::button(self.last_mouse.0, self.last_mouse.1, right, false);
                self.hands[h].mouse_down = None;
            }
        }
        self.update_cursor(h);
    }

    fn zoom_tick(&mut self) {
        let Some(z) = &self.zoom else { return };
        let (Some(a), Some(b)) = (self.hands[0].raw, self.hands[1].raw) else { return };
        let Some(i) = self.panel_idx(z.panel) else { return };
        let w = (z.w0 * (a.pos() - b.pos()).len() / z.d0).clamp(0.1, 8.0);
        if (w - self.panels[i].width_m).abs() > 0.002 {
            self.panels[i].width_m = w;
            self.vr.set_width(self.panels[i].handle, w);
            self.apply_panel_tf(i);
        }
    }

    /// Ring cursor on whatever the hand points at (none while holding).
    fn update_cursor(&mut self, h: usize) {
        let hand = &self.hands[h];
        let show = hand.holding.is_none() && hand.target.is_some() && hand.aim.is_some();
        if !show {
            if hand.cursor_on {
                self.vr.hide(hand.cursor);
                self.hands[h].cursor_on = false;
            }
            return;
        }
        let (Some(t), Some(aim), dist, cursor, cursor_w, cursor_on) =
            (hand.target, hand.aim, hand.dist, hand.cursor, hand.cursor_w, hand.cursor_on)
        else {
            return;
        };
        let Some(mut tf) = self.target_tf(&t) else { return };
        let hit = aim.pos() + aim.forward() * dist;
        tf.set_pos(hit + tf.col(2) * 0.003);
        // Grow slightly with distance so it stays visible far away.
        let w = (0.009 + 0.008 * dist).clamp(0.01, 0.05);
        if (w - cursor_w).abs() / cursor_w > 0.08 {
            self.vr.set_width(cursor, w);
            self.hands[h].cursor_w = w;
        }
        self.vr.set_transform_absolute(cursor, &tf);
        if !cursor_on {
            self.vr.show(cursor);
            self.hands[h].cursor_on = true;
        }
    }

    // ------------------------------------------------------------- wrist

    fn slow_tick(&mut self) {
        for h in 0..2 {
            let dev = self.vr.pose_device(self.act.aim, self.hands[h].src).unwrap_or_else(|| self.vr.controller_index(h == LEFT));
            if dev != self.hands[h].dev {
                self.hands[h].dev = dev;
                if h == self.wrist_hand() {
                    self.wrist_key.clear(); // re-attach the wrist widget
                }
                // Panels carried on this hand follow its new device index.
                for i in 0..self.panels.len() {
                    if self.panels[i].attached.map(|a| a.0) == Some(h) {
                        self.apply_panel_tf(i);
                    }
                }
            }
        }
        if self.layout_dirty {
            self.layout_dirty = false;
            self.save_layout();
        }
        if let Some(rx) = &self.admin_job {
            if let Ok(r) = rx.try_recv() {
                self.admin_job = None;
                self.admin_task_done(r);
            }
        }
        // Windows that went away take their panel with them.
        let dead: Vec<u64> = self.panels.iter().filter(|p| !p.src.alive()).map(|p| p.id).collect();
        for id in dead {
            self.close_panel(id);
        }
        let mut drain = Event::default();
        for p in &self.panels {
            while self.vr.poll_overlay_event(p.handle, &mut drain) {}
            while self.vr.poll_overlay_event(p.tb, &mut drain) {}
        }
        while self.vr.poll_overlay_event(self.menu, &mut drain) {}
        while self.vr.poll_overlay_event(self.kb.handle, &mut drain) {}
        while self.vr.poll_overlay_event(self.settings, &mut drain) {}

        let Some(wh) = self.wrist else { return };
        let wrist_hand = self.wrist_hand();
        let dev = self.hands[wrist_hand].dev;
        if dev == INVALID_DEVICE {
            self.vr.hide(wh);
            self.wrist_vis = false;
            self.wrist_key.clear();
            return;
        }
        if self.wrist_rel.is_none() {
            // First sight of the left hand: derive the default spot from the aim pose.
            if let (Some(raw), Some(aim)) = (self.hands[wrist_hand].raw, self.hands[wrist_hand].aim) {
                self.wrist_rel = Some(raw.inv_rigid().mul(&aim).mul(&wrist_default_local(wrist_hand)));
                self.wrist_key.clear();
            } else {
                return;
            }
        }
        if self.wrist_key.is_empty() && self.wrist_held.is_none() {
            if let Some(rel) = self.wrist_rel {
                self.vr.set_transform_device_relative(wh, dev, &rel);
            }
        }
        if self.battery_at.elapsed() >= Duration::from_secs(30) {
            self.battery_at = Instant::now();
            let devs = [('L', self.hands[LEFT].dev), ('R', self.hands[RIGHT].dev), ('H', 0u32)];
            self.batteries = devs
                .iter()
                .filter(|(_, d)| *d != INVALID_DEVICE && self.vr.prop_bool(*d, vr::prop::PROVIDES_BATTERY).unwrap_or(false))
                .map(|&(l, d)| (l, self.vr.prop_f32(d, vr::prop::BATTERY), self.vr.prop_bool(d, vr::prop::IS_CHARGING).unwrap_or(false)))
                .collect();
        }
        let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        let time = if self.cfg.wrist_24h {
            format!("{:02}:{:02}", t.wHour, t.wMinute)
        } else {
            format!("{}:{:02}", (t.wHour + 11) % 12 + 1, t.wMinute)
        };
        let key =
            format!("{dev}|{time}|{:?}", self.batteries.iter().map(|b| (b.0, b.1.map(|v| (v * 100.) as i32), b.2)).collect::<Vec<_>>());
        if key != self.wrist_key {
            self.wrist_base = Some(ui::wrist_base(&ui::WristInfo { time, batteries: self.batteries.clone() }));
            self.wrist_key = key;
            self.wrist_next_frame = Instant::now(); // repaint now if visible
        }
    }

    /// Like a real watch: the widget appears when the back of your hand turns
    /// toward your eyes, fades with the angle, and hides otherwise.
    fn update_wrist_visibility(&mut self) {
        let Some(wh) = self.wrist else { return };
        let (Some(tf), Some(head)) = (self.wrist_tf(), self.hmd()) else { return };
        let to_eye = (head.pos() - tf.pos()).norm();
        let facing = tf.col(2).dot(to_eye);
        let pointed = self.hands[1 - self.wrist_hand()].target == Some(Target::Wrist);
        let forced = Instant::now() < self.wrist_force_until;
        let want = forced || self.wrist_held.is_some() || pointed || facing > if self.wrist_vis { 0.25 } else { 0.4 };
        if want != self.wrist_vis {
            self.wrist_vis = want;
            if want {
                self.wrist_next_frame = Instant::now();
                self.vr.set_alpha(wh, 0.0);
                self.wrist_alpha = 0.0;
                self.vr.show(wh);
            } else {
                self.vr.hide(wh);
            }
        }
        if want && !self.tweens.iter().any(|t| t.handle == wh) {
            let a = if forced || self.wrist_held.is_some() || pointed { 1.0 } else { ((facing - 0.25) / 0.3).clamp(0.0, 1.0) };
            if (a - self.wrist_alpha).abs() > 0.06 || (a == 1.0 && self.wrist_alpha != 1.0) {
                self.vr.set_alpha(wh, a);
                self.wrist_alpha = a;
            }
        }
    }

    fn animate_wrist(&mut self) -> Option<Instant> {
        let wh = self.wrist?;
        if !self.wrist_vis {
            return None;
        }
        let now = Instant::now();
        if now >= self.wrist_next_frame {
            if let Some(base) = &self.wrist_base {
                let f = ui::wrist_frame(base, (now - self.t0).as_secs_f32());
                self.set_image(wh, &f);
            }
            self.wrist_next_frame = now + WRIST_FRAME;
        }
        Some(self.wrist_next_frame)
    }

    /// Start a pop-in: width overshoots a little and settles, alpha fades in.
    fn pop(&mut self, handle: Handle, width: f32, alpha: f32) {
        self.tweens.retain(|t| t.handle != handle);
        self.vr.set_width(handle, width * 0.2);
        self.vr.set_alpha(handle, 0.0);
        self.tweens.push(Tween { handle, width, alpha, start: Instant::now() });
    }

    /// Where the UFO is heading: just above the back of the left hand, or (no
    /// wrist / no hand) a spot in front of the user.
    fn boot_target(&self) -> Option<(V3, bool)> {
        if self.wrist.is_some() {
            if let Some(tf) = self.wrist_tf() {
                return Some((tf.pos() + tf.col(2) * 0.07, true));
            }
        }
        let head = self.hmd()?;
        let mut f = head.forward();
        f.1 = 0.0;
        Some((head.pos() + f.norm() * 0.8 - V3(0., 0.15, 0.), false))
    }

    fn start_boot(&mut self) {
        self.boot_pending = false;
        let Some(head) = self.hmd() else { return };
        let Ok(handle) = self.vr.create_overlay("ufo.boot", "UFO hello") else { return };
        let mut f = head.forward();
        f.1 = 0.0;
        let f = f.norm();
        let right = f.cross(V3::UP).norm();
        // Enter from high up and a little to the right, far away.
        let from = head.pos() + f * 2.2 + V3::UP * 0.9 + right * 0.8;
        self.vr.set_sort_order(handle, 40);
        self.vr.set_width(handle, 0.05);
        let now = Instant::now();
        self.boot = Some(Boot { handle, start: now, from, next_frame: now, beamed: false });
        log!("boot flight started");
    }

    fn run_boot(&mut self) {
        // Wait (up to 1.5 s) for the left hand so the UFO can land on the wrist.
        let hand_ready = self.wrist.is_none() || self.wrist_tf().is_some() || (Instant::now() - self.t0) > Duration::from_millis(1500);
        if self.boot_pending && self.hmd().is_some() && hand_ready {
            self.start_boot();
        }
        let Some(b) = &self.boot else { return };
        let now = Instant::now();
        if now < b.next_frame {
            return;
        }
        let (handle, start, from, beamed) = (b.handle, b.start, b.from, b.beamed);
        let t = (now - start).as_secs_f32();
        let (Some(head), Some((target, to_wrist))) = (self.hmd(), self.boot_target()) else {
            self.end_boot();
            return;
        };
        if t >= BOOT_END {
            self.end_boot();
            return;
        }
        // Curved approach: a control point above the midpoint gives a swoop.
        let k = ease_in_out(t / BOOT_FLY);
        let ctrl = (from + target) * 0.5 + V3::UP * 0.5;
        let pos = from * ((1.0 - k) * (1.0 - k)) + ctrl * (2.0 * k * (1.0 - k)) + target * (k * k);
        // Wobble while flying, settle while hovering.
        let wob = if t < BOOT_FLY { (t * 9.0).sin() * 14.0 * (1.0 - k) + 8.0 * (1.0 - k) } else { (t * 6.0).sin() * 3.0 };
        let (sr, cr) = wob.to_radians().sin_cos();
        let roll = Mat34::from_axes(V3(cr, sr, 0.), V3(-sr, cr, 0.), V3(0., 0., 1.), V3::default());
        let tf = Mat34::facing(pos, head.pos()).mul(&roll);
        let width = 0.30 + (0.09 - 0.30) * k;
        let beam = t >= BOOT_FLY && to_wrist;
        let mut c = ui::Canvas::new(256, 256, [0, 0, 0, 0]);
        ui::mascot(&mut c, 128.0, 96.0, 200.0, t * 1.6, beam);
        let alpha = if t < 0.15 {
            t / 0.15
        } else if t > BOOT_END - 0.3 {
            (BOOT_END - t) / 0.3
        } else {
            1.0
        };
        self.set_image(handle, &c);
        self.vr.set_width(handle, width);
        self.vr.set_alpha(handle, alpha.clamp(0.0, 1.0));
        // The overlay centre is the canvas centre; the saucer sits higher, so
        // drop the quad so the saucer (not the beam) lands on target.
        let mut tf = tf;
        tf.set_pos(pos - tf.col(1) * (width * (128.0 - 96.0) / 256.0));
        self.vr.set_transform_absolute(handle, &tf);
        if t == 0.0 || self.boot.as_ref().is_some_and(|b| b.next_frame == b.start) {
            self.vr.show(handle);
        }
        if !beamed && to_wrist && t >= BOOT_BEAM_AT {
            if let Some(wh) = self.wrist {
                self.wrist_force_until = now + Duration::from_millis(2500);
                self.wrist_vis = true;
                self.wrist_next_frame = now;
                self.vr.show(wh);
                self.pop(wh, self.cfg.wrist_width_m, 1.0);
                self.haptic(self.wrist_hand(), 0.25);
            }
        }
        if let Some(b) = &mut self.boot {
            b.next_frame = now + BOOT_FRAME;
            b.beamed |= to_wrist && t >= BOOT_BEAM_AT;
        }
    }

    fn end_boot(&mut self) {
        if let Some(b) = self.boot.take() {
            self.vr.destroy_overlay(b.handle);
            self.images.remove(&b.handle);
            log!("boot flight done (landed on wrist: {})", b.beamed);
        }
        self.boot_pending = false;
    }

    fn run_tweens(&mut self) {
        if self.tweens.is_empty() {
            return;
        }
        let now = Instant::now();
        for t in &self.tweens {
            let k = ((now - t.start).as_secs_f32() / POP.as_secs_f32()).min(1.0);
            self.vr.set_width(t.handle, t.width * (0.2 + 0.8 * ease_out_back(k)));
            self.vr.set_alpha(t.handle, t.alpha * (k * 2.0).min(1.0));
        }
        self.tweens.retain(|t| now - t.start < POP);
    }

    // -------------------------------------------------------------- loop

    /// Grab new frames, submit the ones whose rate slot has come.
    /// Returns the next instant a panel wants servicing.
    fn service_panels(&mut self) -> Option<Instant> {
        let now = Instant::now();
        let mut next: Option<Instant> = None;
        let mut submitted = false;
        let mut lost = Vec::new();
        for i in 0..self.panels.len() {
            if self.all_hidden {
                break;
            }
            let fps = self.panels[i].fps(&self.cfg, now);
            let p = &mut self.panels[i];
            if !p.cap.active() {
                if let Err(e) = p.cap.start(&self.gpu, fps) {
                    log!("panel {} capture failed: {e}", p.id);
                    lost.push(p.id);
                    continue;
                }
            }
            p.cap.set_rate(fps);
            match p.cap.grab(&self.gpu) {
                Ok(Grab::Frame) => p.dirty = true,
                Ok(Grab::Resized) => {
                    p.dirty = true;
                    self.apply_panel_tf(i);
                }
                Ok(Grab::None) => {}
                Err(e) => log!("panel {} grab: {e}", p.id),
            }
            let p = &mut self.panels[i];
            if p.dirty {
                let due = p.last_submit + Duration::from_secs_f64(1.0 / fps as f64);
                if now >= due {
                    if !submitted {
                        unsafe { self.gpu.ctx.Flush() };
                        submitted = true;
                    }
                    if let Err(e) = self.vr.set_texture_d3d11(p.handle, p.cap.tex_ptr()) {
                        log!("panel {} {e}", p.id);
                    }
                    p.dirty = false;
                    p.last_submit = now;
                } else {
                    next = Some(next.map_or(due, |n: Instant| n.min(due)));
                }
            }
        }
        for id in lost {
            self.close_panel(id);
        }
        next
    }

    fn run(&mut self) {
        let mut e = Event::default();
        let mut slow_at = Instant::now() - Duration::from_secs(10);
        loop {
            let now = Instant::now();
            let tick = if self.engaged() {
                TICK_ENGAGED
            } else if !self.panels.is_empty() && !self.all_hidden {
                TICK_VISIBLE
            } else {
                TICK_IDLE
            };
            if now >= self.last_input + tick {
                let dt = (now - self.last_input).as_secs_f32().min(0.1);
                self.last_input = now;
                while self.vr.poll_system_event(&mut e) {
                    if e.event_type == ev::QUIT {
                        log!("SteamVR is quitting");
                        return;
                    }
                }
                self.input_tick(dt);
            }
            if self.boot_pending && (Instant::now() - self.t0) > Duration::from_secs(4) {
                self.boot_pending = false;
            }
            if slow_at.elapsed() >= Duration::from_secs(1) {
                slow_at = Instant::now();
                self.slow_tick();
                if cfg!(debug_assertions) {
                    log!(
                        "[dbg] settings_tex={:?} cursor_tex={:?} settings_open={} hover={:?} kb_open={} targets={:?}",
                        self.vr.texture_size(self.settings),
                        self.vr.texture_size(self.hands[1].cursor),
                        self.settings_open,
                        self.settings_hover,
                        self.kb.open,
                        self.hands.iter().map(|h| (h.target, h.cursor_on)).collect::<Vec<_>>()
                    );
                    if let (Some(tf), Some(head)) = (self.wrist_tf(), self.hmd()) {
                        log!(
                            "[dbg] wrist at {:?} facing={:.2} vis={}",
                            tf.pos(),
                            tf.col(2).dot((head.pos() - tf.pos()).norm()),
                            self.wrist_vis
                        );
                    }
                    for p in &self.panels {
                        log!(
                            "[dbg] panel {} tex={:?} tb_on={} wrist_vis={} hands={:?}",
                            p.id,
                            self.vr.texture_size(p.handle),
                            p.tb_on,
                            self.wrist_vis,
                            self.hands.iter().map(|h| (h.dev, h.target, h.cursor_on)).collect::<Vec<_>>()
                        );
                    }
                }
            }
            self.run_boot();
            self.run_tweens();
            let wrist_due = self.animate_wrist();
            let panel_due = self.service_panels();

            let mut wake_at = self.last_input + tick;
            for d in [panel_due, wrist_due].into_iter().flatten() {
                wake_at = wake_at.min(d);
            }
            let ms = wake_at.saturating_duration_since(Instant::now()).as_millis() as u32;
            if ms > 0 {
                // Sleeps until the next input tick, a due submit, or a new frame.
                unsafe { WaitForSingleObject(self.gpu.wake, ms) };
            }
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.save_layout();
        for p in &self.panels {
            self.vr.destroy_overlay(p.handle);
            self.vr.destroy_overlay(p.tb);
        }
        for h in &self.hands {
            self.vr.destroy_overlay(h.cursor);
        }
        self.vr.destroy_overlay(self.menu);
        self.vr.destroy_overlay(self.kb.handle);
        self.vr.destroy_overlay(self.settings);
        if let Some(b) = &self.boot {
            self.vr.destroy_overlay(b.handle);
        }
        if let Some(w) = self.wrist {
            self.vr.destroy_overlay(w);
        }
    }
}

/// Write the SteamVR app manifest next to the exe and return its path.
fn write_manifest() -> Result<PathBuf, String> {
    let dir = exe_dir();
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let manifest = dir.join("ufoverlay.vrmanifest");
    // App image for SteamVR's library / startup list.
    let image = dir.join("ufoverlay.png");
    std::fs::write(&image, include_bytes!("../assets/ufoverlay.png")).map_err(|e| e.to_string())?;
    let image = image.to_string_lossy().replace('\\', "/");
    let json = format!(
        r#"{{
  "source": "builtin",
  "applications": [{{
    "app_key": "{APP_KEY}",
    "launch_type": "binary",
    "binary_path_windows": "{}",
    "is_dashboard_overlay": true,
    "image_path": "{image}",
    "action_manifest_path": "input/actions.json",
    "strings": {{ "en_us": {{ "name": "UFOverlay", "description": "Ultra Fast Overlay" }} }}
  }}]
}}
"#,
        exe.file_name().unwrap().to_string_lossy()
    );
    std::fs::write(&manifest, json).map_err(|e| e.to_string())?;
    Ok(manifest)
}

fn install() -> Result<(), String> {
    let manifest = write_manifest()?;
    let vr = Vr::init()?;
    vr.register_manifest(&manifest.to_string_lossy(), APP_KEY)?;
    log!("registered {} with SteamVR (auto-launch on)", manifest.display());
    Ok(())
}

static INSTANCE: Mutex<Option<usize>> = Mutex::new(None);

/// One UFOverlay at a time. Waits briefly so an elevated relaunch can take over
/// from the copy that is just exiting.
fn single_instance() -> bool {
    use windows::core::w;
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;
    for _ in 0..30 {
        if let Ok(h) = unsafe { CreateMutexW(None, true, w!("Local\\UFOverlay.single")) } {
            if unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
                *INSTANCE.lock().unwrap() = Some(h.0 as usize);
                return true;
            }
            unsafe {
                let _ = CloseHandle(h);
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

fn release_single_instance() {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Threading::ReleaseMutex;
    if let Some(h) = INSTANCE.lock().unwrap().take() {
        unsafe {
            let _ = ReleaseMutex(HANDLE(h as *mut _));
            let _ = CloseHandle(HANDLE(h as *mut _));
        }
    }
}

fn main() {
    let boot = Instant::now();
    let dir = exe_dir();
    if let Ok(f) = File::create(dir.join("ufoverlay.log")) {
        *LOG.lock().unwrap() = Some(f);
    }
    unsafe {
        let _ =
            windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--install") {
        if let Err(e) = install() {
            log!("install failed: {e}");
            std::process::exit(1);
        }
        return;
    }
    // Diagnostic: flip the focused window's IME twice (ends where it started).
    if args.iter().any(|a| a == "--ime-test") {
        let before = ime::state();
        let ok1 = ime::toggle();
        std::thread::sleep(Duration::from_millis(100));
        let mid = ime::state();
        let ok2 = ime::toggle();
        std::thread::sleep(Duration::from_millis(100));
        log!("ime-test: {before:?} -[{ok1}]-> {mid:?} -[{ok2}]-> {:?}", ime::state());
        return;
    }
    let ini = dir.join("ufoverlay.ini");
    let cfg = Config::load(&ini);
    i18n::init(&dir);
    kbd::init(&dir);
    i18n::set(&cfg.language);
    // Admin option: hand over to the elevated copy started by the task.
    // (`--elevated` comes from the task itself, so this can never loop.)
    if cfg.admin && !elevate::is_elevated() && !args.iter().any(|a| a == "--elevated") && elevate::task_exists() && elevate::run_task() {
        log!("admin: relaunched elevated via scheduled task");
        return;
    }
    if !single_instance() {
        log!("UFOverlay is already running");
        return;
    }
    log!("UFOverlay {} — {:?}", env!("CARGO_PKG_VERSION"), cfg);
    let mut app = match App::new(cfg, ini) {
        Ok(a) => a,
        Err(e) => {
            log!("startup failed: {e}");
            std::process::exit(1);
        }
    };
    log!(
        "ready in {} ms (elevated: {}, lang: {}, ime: {:?})",
        boot.elapsed().as_millis(),
        elevate::is_elevated(),
        i18n::code(),
        ime::state()
    );
    if !args.iter().any(|a| a.starts_with("--spawn") || a == "--no-layout") {
        app.restore_layout();
    }
    // Test hooks: open a source right away.
    if args.iter().any(|a| a == "--spawn") {
        if let Some(m) = sources::monitors().into_iter().next() {
            app.spawn(m, None);
        }
    }
    if let Some(pos) = args.iter().position(|a| a == "--spawn-window") {
        if let Some(needle) = args.get(pos + 1) {
            if let Some(w) = sources::windows().into_iter().find(|w| w.label().contains(needle.as_str())) {
                app.spawn(w, None);
            }
        }
    }
    // Test hook: drop the first panel right where the right controller points,
    // so pointer hit-testing can be checked without wearing the headset.
    if args.iter().any(|a| a == "--spawn-aim") {
        app.slow_tick();
        app.input_tick(0.0);
        if let (Some(aim), Some(m)) = (app.hands[RIGHT].aim, sources::monitors().into_iter().next()) {
            app.spawn(m, None);
            let pos = aim.pos() + aim.forward() * 0.8;
            let mut tf = Mat34::facing(pos, aim.pos());
            tf.set_pos(pos);
            let i = app.panels.len() - 1;
            app.panels[i].world = tf;
            app.apply_panel_tf(i);
        }
    }
    // Test hook: settings panel right where the right controller points.
    if args.iter().any(|a| a == "--settings-aim") {
        app.slow_tick();
        app.input_tick(0.0);
        if let Some(aim) = app.hands[RIGHT].aim {
            let pos = aim.pos() + aim.forward() * 0.6;
            let mut tf = Mat34::facing(pos, aim.pos());
            tf.set_pos(pos);
            app.menu_tf = tf;
            app.open_settings();
            log!("settings open, autostart={}", app.autostart);
        }
    }
    // Test hook: keyboard right where the right controller points.
    if args.iter().any(|a| a == "--kb-aim") {
        app.slow_tick();
        app.input_tick(0.0);
        if let Some(aim) = app.hands[RIGHT].aim {
            app.open_keyboard(None);
            let pos = aim.pos() + aim.forward() * 0.6;
            let mut tf = Mat34::facing(pos, aim.pos());
            tf.set_pos(pos);
            app.kb.tf = tf;
            app.vr.set_transform_absolute(app.kb.handle, &tf);
        }
    }
    app.run();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aim_at(yaw_deg: f32) -> Mat34 {
        let (s, c) = yaw_deg.to_radians().sin_cos();
        Mat34::from_axes(V3(c, 0., -s), V3(0., 1., 0.), V3(s, 0., c), V3(0., 1.5, 0.))
    }

    #[test]
    fn smoothing_damps_tremor_but_follows_big_moves() {
        let prev = aim_at(0.0);
        // 0.3° tremor in one 11 ms tick → heavily damped.
        let small = smooth_pose(&prev, &aim_at(0.3), 0.011, 1);
        let moved = small.forward().dot(prev.forward()).clamp(-1., 1.).acos().to_degrees();
        assert!(moved < 0.15, "tremor moved {moved}°");
        // A deliberate 20° flick in one tick → mostly followed.
        let big = smooth_pose(&prev, &aim_at(20.0), 0.011, 1);
        let moved = big.forward().dot(prev.forward()).clamp(-1., 1.).acos().to_degrees();
        assert!(moved > 15.0, "flick only moved {moved}°");
        // Result stays a proper rotation.
        assert!((big.col(0).dot(big.col(1))).abs() < 1e-4 && (big.col(2).len() - 1.0).abs() < 1e-4);
    }

    #[test]
    fn wrist_default_is_mirrored_per_hand() {
        let l = wrist_default_local(LEFT);
        let r = wrist_default_local(RIGHT);
        assert!(l.pos().0 < 0.0 && r.pos().0 > 0.0);
        // Both face out of the back of their hand: −X for left, +X for right.
        assert!(l.col(2).0 < -0.9 && r.col(2).0 > 0.9);
    }
}
