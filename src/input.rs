//! Controller → real Windows mouse/keyboard via SendInput.

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

fn send(inputs: &[INPUT]) {
    unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
}

fn mouse(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32, data: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: data as u32, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    }
}

fn key(vk: VIRTUAL_KEY, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    }
}

/// Screen pixel → the 0..65535 virtual-desktop space SendInput wants.
fn to_abs(x: f32, y: f32) -> (i32, i32) {
    let (vx, vy, vw, vh) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN).max(2),
            GetSystemMetrics(SM_CYVIRTUALSCREEN).max(2),
        )
    };
    (((x - vx as f32) * 65535.0 / (vw - 1) as f32).round() as i32, ((y - vy as f32) * 65535.0 / (vh - 1) as f32).round() as i32)
}

const ABS: MOUSE_EVENT_FLAGS = MOUSE_EVENT_FLAGS(MOUSEEVENTF_MOVE.0 | MOUSEEVENTF_ABSOLUTE.0 | MOUSEEVENTF_VIRTUALDESK.0);

pub fn move_to(x: f32, y: f32) {
    let (x, y) = to_abs(x, y);
    send(&[mouse(ABS, x, y, 0)]);
}

/// `right` = right button, else left.
pub fn button(x: f32, y: f32, right: bool, down: bool) {
    let flag = match (right, down) {
        (true, true) => MOUSEEVENTF_RIGHTDOWN,
        (true, false) => MOUSEEVENTF_RIGHTUP,
        (false, true) => MOUSEEVENTF_LEFTDOWN,
        (false, false) => MOUSEEVENTF_LEFTUP,
    };
    let (x, y) = to_abs(x, y);
    send(&[mouse(ABS, x, y, 0), mouse(flag, 0, 0, 0)]);
}

pub fn wheel(notches_x120: i32) {
    if notches_x120 != 0 {
        send(&[mouse(MOUSEEVENTF_WHEEL, 0, 0, notches_x120)]);
    }
}

/// Bring a window to the front so clicks land on it, not on whatever covers it.
pub fn focus_window(hwnd: HWND) {
    unsafe {
        if GetForegroundWindow() == hwnd {
            return;
        }
        // The foreground lock lets a process that just sent input take focus;
        // a bare ALT tap is the usual way to qualify.
        send(&[key(VK_MENU, 0, KEYBD_EVENT_FLAGS(0)), key(VK_MENU, 0, KEYEVENTF_KEYUP)]);
        let _ = SetForegroundWindow(hwnd);
    }
}

/// Hardware scan code for a virtual key. Some apps and IMEs look at the scan
/// code rather than the VK, so we send both like a real keyboard does.
fn scan(vk: VIRTUAL_KEY) -> u16 {
    unsafe { MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC) as u16 }
}

/// Press one virtual key with optional modifiers (VK codes, so the Korean IME
/// composes Hangul exactly like a physical keyboard).
pub fn key_press(vk: u16, extended: bool, shift: bool, ctrl: bool, alt: bool) {
    let ext = if extended { KEYEVENTF_EXTENDEDKEY } else { KEYBD_EVENT_FLAGS(0) };
    let mut ev = Vec::with_capacity(8);
    let mods: Vec<VIRTUAL_KEY> = [(ctrl, VK_CONTROL), (alt, VK_MENU), (shift, VK_SHIFT)].iter().filter(|m| m.0).map(|m| m.1).collect();
    for &m in &mods {
        ev.push(key(m, scan(m), KEYBD_EVENT_FLAGS(0)));
    }
    let v = VIRTUAL_KEY(vk);
    ev.push(key(v, scan(v), ext));
    ev.push(key(v, scan(v), KEYBD_EVENT_FLAGS(ext.0 | KEYEVENTF_KEYUP.0)));
    for &m in mods.iter().rev() {
        ev.push(key(m, scan(m), KEYEVENTF_KEYUP));
    }
    send(&ev);
}
