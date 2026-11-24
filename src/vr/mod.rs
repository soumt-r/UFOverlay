//! Minimal OpenVR binding through the flat `FnTable:` interfaces.
//!
//! openvr_api.dll is loaded at runtime from the SteamVR install, so there is no
//! C++ toolchain, no bindgen and no static lib. Each interface is an array of
//! function pointers; slot indices are generated from the header
//! (tools/gen_fntable.py) and signatures are declared at each call site below.

#![allow(non_snake_case, dead_code)]

mod fntable;

use std::ffi::{c_char, c_void, CString};
use std::path::PathBuf;

use fntable::{ivrapplications as apps, ivrinput as inp, ivroverlay as ov, ivrsystem as sys};
use windows::core::{PCSTR, PCWSTR};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

pub type Handle = u64;
pub const INVALID_DEVICE: u32 = u32::MAX;

pub const APP_OVERLAY: i32 = 2;
pub const ORIGIN_STANDING: i32 = 1;

pub mod flag {
    pub const SEND_SMOOTH_SCROLL: u32 = 131072;
    pub const IGNORE_TEXTURE_ALPHA: u32 = 4194304;
    pub const CLICK_STABILIZATION: u32 = 134217728;
}

pub mod ev {
    pub const MOUSE_MOVE: u32 = 300;
    pub const MOUSE_DOWN: u32 = 301;
    pub const MOUSE_UP: u32 = 302;
    pub const SCROLL_SMOOTH: u32 = 309;
    pub const OVERLAY_SHOWN: u32 = 500;
    pub const OVERLAY_HIDDEN: u32 = 501;
    pub const QUIT: u32 = 700;
    pub const KEYBOARD_CHAR: u32 = 1201;
}

pub mod prop {
    pub const IS_CHARGING: i32 = 1011;
    pub const BATTERY: i32 = 1012;
    pub const PROVIDES_BATTERY: i32 = 1026;
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Mat34 {
    pub m: [[f32; 4]; 3],
}

impl Mat34 {
    pub fn identity() -> Self {
        Self { m: [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]] }
    }
    /// Rotation about X (pitch) followed by translation.
    pub fn pitch_translate(pitch_deg: f32, t: [f32; 3]) -> Self {
        let (s, c) = pitch_deg.to_radians().sin_cos();
        Self { m: [[1., 0., 0., t[0]], [0., c, -s, t[1]], [0., s, c, t[2]]] }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TextureBounds {
    pub u_min: f32,
    pub v_min: f32,
    pub u_max: f32,
    pub v_max: f32,
}

#[repr(C)]
struct Texture {
    handle: *mut c_void,
    e_type: i32,
    color_space: i32,
}

/// `VREvent_t` on Win64: 12-byte header, padded to 16, then a 48-byte union.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Event {
    pub event_type: u32,
    pub device: u32,
    pub age: f32,
    pub data: [u64; 6],
}
const _: () = assert!(std::mem::size_of::<Event>() == 64);

impl Default for Event {
    fn default() -> Self {
        Self { event_type: 0, device: 0, age: 0., data: [0; 6] }
    }
}

impl Event {
    fn f32_at(&self, i: usize) -> f32 {
        let words: &[u32; 12] = unsafe { &*(self.data.as_ptr() as *const [u32; 12]) };
        f32::from_bits(words[i])
    }
    fn u32_at(&self, i: usize) -> u32 {
        let words: &[u32; 12] = unsafe { &*(self.data.as_ptr() as *const [u32; 12]) };
        words[i]
    }
    /// VREvent_Mouse_t { x, y, button, cursorIndex }
    pub fn mouse(&self) -> (f32, f32, u32) {
        (self.f32_at(0), self.f32_at(1), self.u32_at(2))
    }
    /// VREvent_Scroll_t { xdelta, ydelta, .. }
    pub fn scroll(&self) -> (f32, f32) {
        (self.f32_at(0), self.f32_at(1))
    }
    /// VREvent_Keyboard_t { cNewInput[8], .. }
    pub fn keyboard_text(&self) -> String {
        let bytes: [u8; 8] = self.data[0].to_le_bytes();
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(8);
        String::from_utf8_lossy(&bytes[..end]).into_owned()
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Pose {
    pub m: Mat34,
    pub vel: [f32; 3],
    pub ang_vel: [f32; 3],
    pub result: i32,
    pub valid: bool,
    pub connected: bool,
}
const _: () = assert!(std::mem::size_of::<Pose>() == 80);

#[repr(C)]
#[derive(Default)]
struct DigitalData {
    active: bool,
    origin: u64,
    state: bool,
    changed: bool,
    time: f32,
}
const _: () = assert!(std::mem::size_of::<DigitalData>() == 24);

#[repr(C)]
#[derive(Default)]
struct AnalogData {
    active: bool,
    origin: u64,
    x: f32,
    y: f32,
    z: f32,
    dx: f32,
    dy: f32,
    dz: f32,
    time: f32,
}
const _: () = assert!(std::mem::size_of::<AnalogData>() == 48);

#[repr(C)]
#[derive(Default)]
struct PoseData {
    active: bool,
    origin: u64,
    pose: Pose,
}
const _: () = assert!(std::mem::size_of::<PoseData>() == 96);

#[repr(C)]
struct ActiveSet {
    set: u64,
    restrict: u64,
    secondary: u64,
    pad: u32,
    priority: i32,
}
const _: () = assert!(std::mem::size_of::<ActiveSet>() == 32);

macro_rules! vcall {
    ($tbl:expr, $slot:expr, fn($($t:ty),*) -> $r:ty $(, $a:expr)* ) => {{
        let f: unsafe extern "system" fn($($t),*) -> $r =
            unsafe { std::mem::transmute(*($tbl as *const usize).add($slot)) };
        unsafe { f($($a),*) }
    }};
}

type InitFn = unsafe extern "system" fn(*mut i32, i32, *const c_char) -> u32;
type GetIfaceFn = unsafe extern "system" fn(*const c_char, *mut i32) -> *const c_void;
type ShutdownFn = unsafe extern "system" fn();

pub struct Vr {
    shutdown: ShutdownFn,
    sys: *const c_void,
    ov: *const c_void,
    apps: *const c_void,
    input: *const c_void,
}

fn runtime_dir() -> PathBuf {
    // %LOCALAPPDATA%\openvr\openvrpaths.vrpath -> "runtime": [ "<dir>" ]
    let fallback = PathBuf::from(r"C:\Program Files (x86)\Steam\steamapps\common\SteamVR");
    let Ok(local) = std::env::var("LOCALAPPDATA") else { return fallback };
    let Ok(txt) = std::fs::read_to_string(PathBuf::from(local).join(r"openvr\openvrpaths.vrpath")) else {
        return fallback;
    };
    let Some(at) = txt.find("\"runtime\"") else { return fallback };
    let rest = &txt[at + 9..];
    let Some(q0) = rest.find('[').and_then(|b| rest[b..].find('"').map(|q| b + q + 1)) else {
        return fallback;
    };
    let Some(q1) = rest[q0..].find('"') else { return fallback };
    PathBuf::from(rest[q0..q0 + q1].replace("\\\\", "\\"))
}

fn cstr(s: &str) -> CString {
    CString::new(s).unwrap_or_default()
}

impl Vr {
    pub fn init() -> Result<Self, String> {
        let dll = runtime_dir().join(r"bin\win64\openvr_api.dll");
        let wide: Vec<u16> = dll.as_os_str().encode_wide_nul();
        let module = unsafe { LoadLibraryW(PCWSTR(wide.as_ptr())) }.map_err(|e| format!("cannot load {}: {e}", dll.display()))?;
        let sym = |name: &[u8]| unsafe { GetProcAddress(module, PCSTR(name.as_ptr())) };
        let (Some(init), Some(get), Some(shutdown)) =
            (sym(b"VR_InitInternal2\0"), sym(b"VR_GetGenericInterface\0"), sym(b"VR_ShutdownInternal\0"))
        else {
            return Err("openvr_api.dll is missing expected exports".into());
        };
        let init: InitFn = unsafe { std::mem::transmute(init) };
        let get: GetIfaceFn = unsafe { std::mem::transmute(get) };
        let shutdown: ShutdownFn = unsafe { std::mem::transmute(shutdown) };

        let mut err = 0i32;
        unsafe { init(&mut err, APP_OVERLAY, std::ptr::null()) };
        if err != 0 {
            return Err(format!("VR_Init failed (EVRInitError {err}) — is SteamVR running?"));
        }
        let iface = |v: &str| -> Result<*const c_void, String> {
            let mut e = 0i32;
            let p = unsafe { get(v.as_ptr() as *const c_char, &mut e) };
            if p.is_null() || e != 0 {
                Err(format!("interface {} unavailable ({e})", v.trim_end_matches('\0')))
            } else {
                Ok(p)
            }
        };
        let vr =
            Vr { shutdown, sys: iface(sys::VERSION)?, ov: iface(ov::VERSION)?, apps: iface(apps::VERSION)?, input: iface(inp::VERSION)? };
        Ok(vr)
    }

    // ---- IVRSystem -----------------------------------------------------

    pub fn poll_system_event(&self, e: &mut Event) -> bool {
        vcall!(self.sys, sys::PollNextEvent, fn(*mut Event, u32) -> bool, e, 64)
    }

    /// DXGI adapter index SteamVR renders on.
    pub fn dxgi_adapter_index(&self) -> i32 {
        let mut idx = -1i32;
        vcall!(self.sys, sys::GetDXGIOutputInfo, fn(*mut i32) -> (), &mut idx);
        idx
    }

    pub fn controller_index(&self, left: bool) -> u32 {
        vcall!(self.sys, sys::GetTrackedDeviceIndexForControllerRole, fn(i32) -> u32, if left { 1 } else { 2 })
    }

    pub fn prop_f32(&self, dev: u32, p: i32) -> Option<f32> {
        let mut e = 0i32;
        let v = vcall!(self.sys, sys::GetFloatTrackedDeviceProperty, fn(u32, i32, *mut i32) -> f32, dev, p, &mut e);
        (e == 0).then_some(v)
    }

    /// Raw tracking poses for device indices 0..N (0 = HMD).
    pub fn device_poses(&self, out: &mut [Pose]) {
        vcall!(
            self.sys,
            sys::GetDeviceToAbsoluteTrackingPose,
            fn(i32, f32, *mut Pose, u32) -> (),
            ORIGIN_STANDING,
            0.0,
            out.as_mut_ptr(),
            out.len() as u32
        );
    }

    pub fn prop_bool(&self, dev: u32, p: i32) -> Option<bool> {
        let mut e = 0i32;
        let v = vcall!(self.sys, sys::GetBoolTrackedDeviceProperty, fn(u32, i32, *mut i32) -> bool, dev, p, &mut e);
        (e == 0).then_some(v)
    }

    // ---- IVROverlay ----------------------------------------------------

    fn check(&self, what: &str, e: i32) -> Result<(), String> {
        if e == 0 {
            Ok(())
        } else {
            Err(format!("{what}: EVROverlayError {e}"))
        }
    }

    pub fn create_overlay(&self, key: &str, name: &str) -> Result<Handle, String> {
        let (k, n) = (cstr(key), cstr(name));
        let mut h: Handle = 0;
        let e = vcall!(self.ov, ov::CreateOverlay, fn(*const c_char, *const c_char, *mut Handle) -> i32, k.as_ptr(), n.as_ptr(), &mut h);
        self.check("CreateOverlay", e).map(|_| h)
    }

    pub fn create_dashboard_overlay(&self, key: &str, name: &str) -> Result<(Handle, Handle), String> {
        let (k, n) = (cstr(key), cstr(name));
        let (mut main, mut thumb): (Handle, Handle) = (0, 0);
        let e = vcall!(
            self.ov,
            ov::CreateDashboardOverlay,
            fn(*const c_char, *const c_char, *mut Handle, *mut Handle) -> i32,
            k.as_ptr(),
            n.as_ptr(),
            &mut main,
            &mut thumb
        );
        self.check("CreateDashboardOverlay", e).map(|_| (main, thumb))
    }

    pub fn destroy_overlay(&self, h: Handle) {
        vcall!(self.ov, ov::DestroyOverlay, fn(Handle) -> i32, h);
    }

    pub fn set_flag(&self, h: Handle, flag: u32, on: bool) {
        vcall!(self.ov, ov::SetOverlayFlag, fn(Handle, u32, bool) -> i32, h, flag, on);
    }

    pub fn set_alpha(&self, h: Handle, a: f32) {
        vcall!(self.ov, ov::SetOverlayAlpha, fn(Handle, f32) -> i32, h, a);
    }

    pub fn set_sort_order(&self, h: Handle, o: u32) {
        vcall!(self.ov, ov::SetOverlaySortOrder, fn(Handle, u32) -> i32, h, o);
    }

    pub fn set_width(&self, h: Handle, m: f32) {
        vcall!(self.ov, ov::SetOverlayWidthInMeters, fn(Handle, f32) -> i32, h, m);
    }

    pub fn set_curvature(&self, h: Handle, c: f32) {
        vcall!(self.ov, ov::SetOverlayCurvature, fn(Handle, f32) -> i32, h, c);
    }

    pub fn set_texture_bounds(&self, h: Handle, b: TextureBounds) {
        let mut b = b;
        vcall!(self.ov, ov::SetOverlayTextureBounds, fn(Handle, *mut TextureBounds) -> i32, h, &mut b);
    }

    pub fn set_transform_absolute(&self, h: Handle, m: &Mat34) {
        let mut m = *m;
        vcall!(self.ov, ov::SetOverlayTransformAbsolute, fn(Handle, i32, *mut Mat34) -> i32, h, ORIGIN_STANDING, &mut m);
    }

    pub fn set_transform_device_relative(&self, h: Handle, dev: u32, m: &Mat34) {
        let mut m = *m;
        vcall!(self.ov, ov::SetOverlayTransformTrackedDeviceRelative, fn(Handle, u32, *mut Mat34) -> i32, h, dev, &mut m);
    }

    /// World transform of a point on the overlay (mouse-scale coordinates).
    pub fn transform_for_coords(&self, h: Handle, at: Vec2) -> Option<Mat34> {
        let mut m = Mat34::default();
        let e = vcall!(
            self.ov,
            ov::GetTransformForOverlayCoordinates,
            fn(Handle, i32, Vec2, *mut Mat34) -> i32,
            h,
            ORIGIN_STANDING,
            at,
            &mut m
        );
        (e == 0).then_some(m)
    }

    pub fn show(&self, h: Handle) {
        vcall!(self.ov, ov::ShowOverlay, fn(Handle) -> i32, h);
    }

    pub fn hide(&self, h: Handle) {
        vcall!(self.ov, ov::HideOverlay, fn(Handle) -> i32, h);
    }

    pub fn is_visible(&self, h: Handle) -> bool {
        vcall!(self.ov, ov::IsOverlayVisible, fn(Handle) -> bool, h)
    }

    pub fn poll_overlay_event(&self, h: Handle, e: &mut Event) -> bool {
        vcall!(self.ov, ov::PollNextOverlayEvent, fn(Handle, *mut Event, u32) -> bool, h, e, 64)
    }

    pub fn set_input_mouse(&self, h: Handle, w: f32, hgt: f32) {
        vcall!(self.ov, ov::SetOverlayInputMethod, fn(Handle, i32) -> i32, h, 1);
        let mut s = Vec2 { x: w, y: hgt };
        vcall!(self.ov, ov::SetOverlayMouseScale, fn(Handle, *mut Vec2) -> i32, h, &mut s);
    }

    /// Submit a D3D11 texture (ID3D11Texture2D*). The compositor snapshots it.
    pub fn set_texture_d3d11(&self, h: Handle, tex: *mut c_void) -> Result<(), String> {
        let mut t = Texture { handle: tex, e_type: 0, color_space: 1 };
        let e = vcall!(self.ov, ov::SetOverlayTexture, fn(Handle, *mut Texture) -> i32, h, &mut t);
        self.check("SetOverlayTexture", e)
    }

    pub fn texture_size(&self, h: Handle) -> Option<(u32, u32)> {
        let (mut w, mut hh) = (0u32, 0u32);
        let e = vcall!(self.ov, ov::GetOverlayTextureSize, fn(Handle, *mut u32, *mut u32) -> i32, h, &mut w, &mut hh);
        (e == 0).then_some((w, hh))
    }

    pub fn show_keyboard_for(&self, h: Handle) {
        let empty = cstr("");
        vcall!(
            self.ov,
            ov::ShowKeyboardForOverlay,
            fn(Handle, i32, i32, u32, *const c_char, u32, *const c_char, u64) -> i32,
            h,
            0,
            0,
            1,
            empty.as_ptr(),
            1024,
            empty.as_ptr(),
            0
        );
    }

    // ---- IVRInput -------------------------------------------------------

    pub fn set_action_manifest(&self, path: &str) -> Result<(), String> {
        let p = cstr(path);
        let e = vcall!(self.input, inp::SetActionManifestPath, fn(*const c_char) -> i32, p.as_ptr());
        if e == 0 {
            Ok(())
        } else {
            Err(format!("SetActionManifestPath: EVRInputError {e}"))
        }
    }

    pub fn action_set(&self, name: &str) -> u64 {
        let n = cstr(name);
        let mut h = 0u64;
        vcall!(self.input, inp::GetActionSetHandle, fn(*const c_char, *mut u64) -> i32, n.as_ptr(), &mut h);
        h
    }

    pub fn action(&self, name: &str) -> u64 {
        let n = cstr(name);
        let mut h = 0u64;
        vcall!(self.input, inp::GetActionHandle, fn(*const c_char, *mut u64) -> i32, n.as_ptr(), &mut h);
        h
    }

    pub fn input_source(&self, path: &str) -> u64 {
        let n = cstr(path);
        let mut h = 0u64;
        vcall!(self.input, inp::GetInputSourceHandle, fn(*const c_char, *mut u64) -> i32, n.as_ptr(), &mut h);
        h
    }

    /// Active-set priority that lets an overlay app take inputs away from the
    /// game (k_nActionSetOverlayGlobalPriorityMin).
    pub const PRIORITY_OVER_GAME: i32 = 0x0100_0000;

    /// Activate `set` once per hand: `(input source, priority)`. A hand whose
    /// priority is PRIORITY_OVER_GAME hides its bound buttons from the game.
    pub fn update_actions(&self, set: u64, hands: &[(u64, i32)]) -> i32 {
        let mut sets: Vec<ActiveSet> =
            hands.iter().map(|&(restrict, priority)| ActiveSet { set, restrict, secondary: 0, pad: 0, priority }).collect();
        vcall!(self.input, inp::UpdateActionState, fn(*mut ActiveSet, u32, u32) -> i32, sets.as_mut_ptr(), 32, sets.len() as u32)
    }

    /// (pressed, changed since last update)
    pub fn digital(&self, action: u64, src: u64) -> (bool, bool) {
        let mut d = DigitalData::default();
        let e = vcall!(self.input, inp::GetDigitalActionData, fn(u64, *mut DigitalData, u32, u64) -> i32, action, &mut d, 24, src);
        if e != 0 || !d.active {
            (false, false)
        } else {
            (d.state, d.changed)
        }
    }

    pub fn analog(&self, action: u64, src: u64) -> (f32, f32) {
        let mut d = AnalogData::default();
        let e = vcall!(self.input, inp::GetAnalogActionData, fn(u64, *mut AnalogData, u32, u64) -> i32, action, &mut d, 48, src);
        if e != 0 || !d.active {
            (0., 0.)
        } else {
            (d.x, d.y)
        }
    }

    pub fn pose(&self, action: u64, src: u64) -> Option<Mat34> {
        let mut d = PoseData::default();
        let e = vcall!(
            self.input,
            inp::GetPoseActionDataRelativeToNow,
            fn(u64, i32, f32, *mut PoseData, u32, u64) -> i32,
            action,
            ORIGIN_STANDING,
            0.0,
            &mut d,
            96,
            src
        );
        (e == 0 && d.active && d.pose.valid).then_some(d.pose.m)
    }

    /// Tracked-device index behind a pose action for this hand (works even when
    /// controller roles are unassigned, e.g. some streaming setups).
    pub fn pose_device(&self, action: u64, src: u64) -> Option<u32> {
        #[repr(C)]
        struct OriginInfo {
            path: u64,
            index: u32,
            name: [u8; 128],
        }
        const _: () = assert!(std::mem::size_of::<OriginInfo>() == 144);
        let mut d = PoseData::default();
        let e = vcall!(
            self.input,
            inp::GetPoseActionDataRelativeToNow,
            fn(u64, i32, f32, *mut PoseData, u32, u64) -> i32,
            action,
            ORIGIN_STANDING,
            0.0,
            &mut d,
            96,
            src
        );
        if e != 0 || !d.active || d.origin == 0 {
            return None;
        }
        let mut info = OriginInfo { path: 0, index: INVALID_DEVICE, name: [0; 128] };
        let e = vcall!(self.input, inp::GetOriginTrackedDeviceInfo, fn(u64, *mut OriginInfo, u32) -> i32, d.origin, &mut info, 144);
        (e == 0 && info.index != INVALID_DEVICE).then_some(info.index)
    }

    pub fn haptic(&self, action: u64, src: u64, secs: f32, freq: f32, amp: f32) {
        vcall!(self.input, inp::TriggerHapticVibrationAction, fn(u64, f32, f32, f32, f32, u64) -> i32, action, 0.0, secs, freq, amp, src);
    }

    // ---- IVRApplications -----------------------------------------------

    pub fn auto_launch(&self, app_key: &str) -> bool {
        let k = cstr(app_key);
        let installed = vcall!(self.apps, apps::IsApplicationInstalled, fn(*const c_char) -> bool, k.as_ptr());
        installed && vcall!(self.apps, apps::GetApplicationAutoLaunch, fn(*const c_char) -> bool, k.as_ptr())
    }

    pub fn set_auto_launch(&self, app_key: &str, on: bool) -> Result<(), String> {
        let k = cstr(app_key);
        let e = vcall!(self.apps, apps::SetApplicationAutoLaunch, fn(*const c_char, bool) -> i32, k.as_ptr(), on);
        if e == 0 {
            Ok(())
        } else {
            Err(format!("SetApplicationAutoLaunch: EVRApplicationError {e}"))
        }
    }

    pub fn register_manifest(&self, path: &str, app_key: &str) -> Result<(), String> {
        let p = cstr(path);
        let e = vcall!(self.apps, apps::AddApplicationManifest, fn(*const c_char, bool) -> i32, p.as_ptr(), false);
        if e != 0 {
            return Err(format!("AddApplicationManifest: EVRApplicationError {e}"));
        }
        let k = cstr(app_key);
        let e = vcall!(self.apps, apps::SetApplicationAutoLaunch, fn(*const c_char, bool) -> i32, k.as_ptr(), true);
        if e != 0 {
            return Err(format!("SetApplicationAutoLaunch: EVRApplicationError {e}"));
        }
        Ok(())
    }
}

impl Drop for Vr {
    fn drop(&mut self) {
        unsafe { (self.shutdown)() };
    }
}

trait EncodeWideNul {
    fn encode_wide_nul(&self) -> Vec<u16>;
}

impl EncodeWideNul for std::ffi::OsStr {
    fn encode_wide_nul(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().chain(std::iter::once(0)).collect()
    }
}
