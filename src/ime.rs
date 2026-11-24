//! Reads (and flips) the input-method state of the foreground window, so the
//! VR keyboard can show 한/영 or あ/A correctly and switch it reliably.
//!
//! Works across processes through the IME's default window
//! (WM_IME_CONTROL), the same trick IME indicators use.

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Input::Ime::ImmGetDefaultIMEWnd;
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout;
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId, SendMessageTimeoutW, SMTO_ABORTIFHUNG};

const WM_IME_CONTROL: u32 = 0x0283;
const IMC_GETCONVERSIONMODE: usize = 0x0001;
const IMC_SETCONVERSIONMODE: usize = 0x0002;
const IMC_GETOPENSTATUS: usize = 0x0005;
const IMC_SETOPENSTATUS: usize = 0x0006;
const IME_CMODE_NATIVE: usize = 0x0001;

/// Korean IMEs switch 한/영 through the conversion mode; others (Japanese,
/// Chinese, …) through the open status.
const LANG_KOREAN: u16 = 0x12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImeState {
    /// Primary language id of the foreground window's input language
    /// (0x12 Korean, 0x11 Japanese, 0x09 English, …).
    pub lcid: u16,
    /// Native input (Hangul / kana / …) is on, vs. Latin.
    pub native: bool,
}

fn ime_window() -> Option<(HWND, u16)> {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() {
            return None;
        }
        let tid = GetWindowThreadProcessId(fg, None);
        let lcid = ((GetKeyboardLayout(tid).0 as usize) & 0x3FF) as u16;
        Some((ImmGetDefaultIMEWnd(fg), lcid))
    }
}

fn ask(ime: HWND, cmd: usize, arg: isize) -> Option<usize> {
    let mut out = 0usize;
    let ok = unsafe { SendMessageTimeoutW(ime, WM_IME_CONTROL, WPARAM(cmd), LPARAM(arg), SMTO_ABORTIFHUNG, 60, Some(&mut out)) };
    (ok.0 != 0).then_some(out)
}

pub fn state() -> ImeState {
    let Some((ime, lcid)) = ime_window() else { return ImeState { lcid: 0x09, native: false } };
    if ime.is_invalid() {
        return ImeState { lcid, native: false };
    }
    let mode = ask(ime, IMC_GETCONVERSIONMODE, 0).unwrap_or(0);
    let native = if lcid == LANG_KOREAN {
        // Korean IME: conversion mode's NATIVE bit is 한글.
        mode & IME_CMODE_NATIVE != 0
    } else {
        // Japanese & co.: the IME must be open *and* in a native mode.
        ask(ime, IMC_GETOPENSTATUS, 0).unwrap_or(0) != 0 && mode & IME_CMODE_NATIVE != 0
    };
    ImeState { lcid, native }
}

/// Flip 한/영 or あ/A on the foreground window. Returns false when there is no
/// IME to talk to (the caller then falls back to sending the toggle key).
pub fn toggle() -> bool {
    let Some((ime, lcid)) = ime_window() else { return false };
    if ime.is_invalid() {
        return false;
    }
    if lcid == LANG_KOREAN {
        let Some(mode) = ask(ime, IMC_GETCONVERSIONMODE, 0) else { return false };
        // Switching to 한글 on a closed IME context does nothing; open it first.
        if mode & IME_CMODE_NATIVE == 0 && ask(ime, IMC_GETOPENSTATUS, 0) == Some(0) {
            ask(ime, IMC_SETOPENSTATUS, 1);
        }
        ask(ime, IMC_SETCONVERSIONMODE, (mode ^ IME_CMODE_NATIVE) as isize).is_some()
    } else {
        let Some(open) = ask(ime, IMC_GETOPENSTATUS, 0) else { return false };
        ask(ime, IMC_SETOPENSTATUS, (open == 0) as isize).is_some()
    }
}
