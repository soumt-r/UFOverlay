//! What can be captured: whole monitors and top-level app windows.

use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW};
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Debug)]
pub enum Source {
    Monitor { hmon: HMONITOR, index: usize, rect: RECT },
    Window { hwnd: HWND, title: String },
}

impl Source {
    pub fn label(&self) -> String {
        match self {
            Source::Monitor { index, rect, .. } => {
                format!("{} {}  ·  {}×{}", crate::i18n::tr("menu.monitor"), index + 1, rect.right - rect.left, rect.bottom - rect.top)
            }
            Source::Window { title, .. } => title.clone(),
        }
    }

    /// Screen rectangle the captured image corresponds to (physical px).
    pub fn screen_rect(&self) -> Option<RECT> {
        match self {
            Source::Monitor { rect, .. } => Some(*rect),
            Source::Window { hwnd, .. } => window_bounds(*hwnd),
        }
    }

    pub fn alive(&self) -> bool {
        match self {
            Source::Monitor { .. } => true,
            Source::Window { hwnd, .. } => unsafe { IsWindow(Some(*hwnd)).as_bool() },
        }
    }

    pub fn same(&self, o: &Source) -> bool {
        match (self, o) {
            (Source::Monitor { hmon: a, .. }, Source::Monitor { hmon: b, .. }) => a == b,
            (Source::Window { hwnd: a, .. }, Source::Window { hwnd: b, .. }) => a == b,
            _ => false,
        }
    }
}

/// (exe file name, window class) — stable across restarts, unlike the HWND.
pub fn window_identity(hwnd: HWND) -> (String, String) {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    let mut exe = String::new();
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            let mut buf = [0u16; 520];
            let mut len = buf.len() as u32;
            if QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut len).is_ok() {
                let full = String::from_utf16_lossy(&buf[..len as usize]);
                exe = full.rsplit('\\').next().unwrap_or("").to_string();
            }
            let _ = CloseHandle(h);
        }
    }
    let mut cls = [0u16; 256];
    let n = unsafe { GetClassNameW(hwnd, &mut cls) };
    (exe, String::from_utf16_lossy(&cls[..n.max(0) as usize]))
}

pub fn window_bounds(hwnd: HWND) -> Option<RECT> {
    let mut r = RECT::default();
    unsafe {
        DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut _ as *mut _, std::mem::size_of::<RECT>() as u32).ok()?;
    }
    Some(r)
}

pub fn monitors() -> Vec<Source> {
    unsafe extern "system" fn cb(h: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> BOOL {
        let out = unsafe { &mut *(lp.0 as *mut Vec<(HMONITOR, RECT, bool)>) };
        let mut mi = MONITORINFOEXW {
            monitorInfo: MONITORINFO { cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32, ..Default::default() },
            ..Default::default()
        };
        if unsafe { GetMonitorInfoW(h, &mut mi as *mut _ as *mut MONITORINFO) }.as_bool() {
            out.push((h, mi.monitorInfo.rcMonitor, mi.monitorInfo.dwFlags & 1 != 0));
        }
        true.into()
    }
    let mut v: Vec<(HMONITOR, RECT, bool)> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut v as *mut _ as isize));
    }
    // Primary first, then left-to-right.
    v.sort_by_key(|(_, r, primary)| (!primary, r.left, r.top));
    v.into_iter().enumerate().map(|(i, (hmon, rect, _))| Source::Monitor { hmon, index: i, rect }).collect()
}

/// Visible, titled, un-cloaked, top-level app windows in Z order.
pub fn windows() -> Vec<Source> {
    unsafe extern "system" fn cb(h: HWND, lp: LPARAM) -> BOOL {
        let out = unsafe { &mut *(lp.0 as *mut Vec<Source>) };
        unsafe {
            if !IsWindowVisible(h).as_bool() || IsIconic(h).as_bool() {
                return true.into();
            }
            if GetWindow(h, GW_OWNER).map(|o| !o.is_invalid()).unwrap_or(false) {
                return true.into();
            }
            let ex = GetWindowLongW(h, GWL_EXSTYLE) as u32;
            if ex & WS_EX_TOOLWINDOW.0 != 0 {
                return true.into();
            }
            let mut cloaked = 0u32;
            let _ = DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut _ as *mut _, 4);
            if cloaked != 0 {
                return true.into();
            }
            let mut buf = [0u16; 256];
            let n = GetWindowTextW(h, &mut buf);
            if n <= 0 {
                return true.into();
            }
            let title = String::from_utf16_lossy(&buf[..n as usize]);
            if title == "Program Manager" {
                return true.into();
            }
            if let Some(r) = super::sources::window_bounds(h) {
                if r.right - r.left < 64 || r.bottom - r.top < 64 {
                    return true.into();
                }
            }
            out.push(Source::Window { hwnd: h, title });
        }
        true.into()
    }
    let mut v = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut v as *mut _ as isize));
    }
    v
}
