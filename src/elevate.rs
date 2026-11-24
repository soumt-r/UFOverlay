//! "Admin windows" option. Windows won't let a normal process click into an
//! elevated window (UIPI), so UFOverlay must itself run elevated. Doing that on
//! every start would mean a UAC prompt you can't see from VR, so instead we
//! register a Task Scheduler entry with "run with highest privileges" once
//! (one UAC prompt, on the PC), and later starts launch through that task
//! silently.

use std::os::windows::process::CommandExt;
use std::process::Command;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Threading::{GetCurrentProcess, GetExitCodeProcess, OpenProcessToken, WaitForSingleObject, INFINITE};
use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};

const TASK: &str = "UFOverlay";
const NO_WINDOW: u32 = 0x0800_0000;

pub fn is_elevated() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut e = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut e as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && e.TokenIsElevated != 0
    }
}

fn schtasks(args: &[&str]) -> bool {
    Command::new("schtasks.exe").args(args).creation_flags(NO_WINDOW).output().map(|o| o.status.success()).unwrap_or(false)
}

pub fn task_exists() -> bool {
    schtasks(&["/query", "/tn", TASK])
}

/// Start the elevated copy through the task (no prompt). True if it launched.
pub fn run_task() -> bool {
    schtasks(&["/run", "/tn", TASK])
}

/// Run schtasks elevated (UAC prompt) and wait for it.
fn schtasks_elevated(params: &str) -> Result<(), String> {
    let wide: Vec<u16> = params.encode_utf16().chain(std::iter::once(0)).collect();
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: w!("schtasks.exe"),
        lpParameters: PCWSTR(wide.as_ptr()),
        nShow: 0,
        ..Default::default()
    };
    unsafe {
        ShellExecuteExW(&mut info).map_err(|e| format!("UAC: {e}"))?;
        if info.hProcess.is_invalid() {
            return Err("no process".into());
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 1u32;
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        if code == 0 {
            Ok(())
        } else {
            Err(format!("schtasks exit {code}"))
        }
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// On-demand task (no triggers) that starts `exe` with highest privileges in
/// the user's session. Also fine on battery — this is a laptop-friendly app.
pub fn create_task(exe: &std::path::Path) -> Result<(), String> {
    let exe_s = exe.to_string_lossy();
    let dir = exe.parent().map(|d| d.to_string_lossy().to_string()).unwrap_or_default();
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>UFOverlay (admin windows option)</Description></RegistrationInfo>
  <Principals><Principal id="Author"><LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author"><Exec><Command>{}</Command><Arguments>--elevated</Arguments><WorkingDirectory>{}</WorkingDirectory></Exec></Actions>
</Task>
"#,
        xml_escape(&exe_s),
        xml_escape(&dir)
    );
    let path = std::env::temp_dir().join("ufoverlay_task.xml");
    let mut bytes = vec![0xFF, 0xFE]; // UTF-16 LE BOM, which schtasks expects
    for u in xml.encode_utf16() {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    let r = schtasks_elevated(&format!("/create /f /tn {TASK} /xml \"{}\"", path.display()));
    let _ = std::fs::remove_file(&path);
    r
}

pub fn delete_task() -> Result<(), String> {
    schtasks_elevated(&format!("/delete /f /tn {TASK}"))
}
