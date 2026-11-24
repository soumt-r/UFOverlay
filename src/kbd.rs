//! Keyboard layouts: `i18n/keyboards/<code>.toml` (standard TOML, read by
//! tinytoml).
//!
//! Like language packs: repo files are compiled in, files in
//! `<exe dir>/i18n/keyboards/` override (same name) or add layouts. The
//! layout shown follows the IME language of the focused window (`@lcid`).
//! Keys are sent as virtual-key codes, so the system IME composes text
//! exactly as with a physical keyboard.

use std::path::Path;
use std::sync::OnceLock;

use crate::tinytoml::{self, Value};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum KeyKind {
    /// Virtual-key code; `ext` = extended key (arrows, Del).
    Vk(u16, bool),
    /// IME toggle (한/영, 半/全): switches the focused window's IME directly.
    Ime,
    Shift,
    Ctrl,
    Alt,
    Close,
}

#[derive(Clone, Debug)]
pub struct KeyDef {
    pub label: &'static str,
    pub shifted: &'static str,
    /// Second character printed on the key (e.g. Hangul jamo).
    pub legend: &'static str,
    pub legend_shifted: &'static str,
    pub kind: KeyKind,
    pub width: f32,
    /// Label came from the layout file (`0xVK=Label`): drawn in the layout font.
    pub custom: bool,
}

#[derive(Debug)]
pub struct Layout {
    pub code: &'static str,
    #[allow(dead_code)] // human-readable; for logs / a future layout picker
    pub name: &'static str,
    pub lcids: Vec<u16>,
    /// (key label, text while native input is on, text while off)
    pub ime: Option<(&'static str, &'static str, &'static str)>,
    pub ime_vk: u16,
    pub font: Option<&'static str>,
    pub legend_big_when_ime: bool,
    pub rows: Vec<Vec<KeyDef>>,
}

pub const ROW_UNITS: f32 = 15.0;

const BUILTIN: &[(&str, &str)] = &[
    ("ko", include_str!("../i18n/keyboards/ko.toml")),
    ("ja", include_str!("../i18n/keyboards/ja.toml")),
    ("en", include_str!("../i18n/keyboards/en.toml")),
];

/// `(file stem, contents)` of every `*.toml` in a drop-in folder.
pub fn user_files(dir: Option<&Path>) -> Vec<(String, String)> {
    let Some(dir) = dir else { return Vec::new() };
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<(String, String)> = rd
        .flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("toml"))
        .filter_map(|e| {
            let stem = e.path().file_stem()?.to_string_lossy().to_string();
            Some((stem, std::fs::read_to_string(e.path()).ok()?))
        })
        .collect();
    out.sort();
    out
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

/// Built-in key names usable in `row` lines.
fn catalog(name: &str) -> Option<(&'static str, &'static str, KeyKind)> {
    use KeyKind::*;
    let vk = |c: u16| Vk(c, false);
    let ext = |c: u16| Vk(c, true);
    Some(match name {
        "esc" => ("Esc", "Esc", vk(0x1B)),
        "grave" => ("`", "~", vk(0xC0)),
        "1" => ("1", "!", vk(0x31)),
        "2" => ("2", "@", vk(0x32)),
        "3" => ("3", "#", vk(0x33)),
        "4" => ("4", "$", vk(0x34)),
        "5" => ("5", "%", vk(0x35)),
        "6" => ("6", "^", vk(0x36)),
        "7" => ("7", "&", vk(0x37)),
        "8" => ("8", "*", vk(0x38)),
        "9" => ("9", "(", vk(0x39)),
        "0" => ("0", ")", vk(0x30)),
        "minus" => ("-", "_", vk(0xBD)),
        "equal" => ("=", "+", vk(0xBB)),
        "backspace" => ("⌫", "⌫", vk(0x08)),
        "tab" => ("Tab", "Tab", vk(0x09)),
        "lbracket" => ("[", "{", vk(0xDB)),
        "rbracket" => ("]", "}", vk(0xDD)),
        "backslash" => ("\\", "|", vk(0xDC)),
        "caps" => ("Caps", "Caps", vk(0x14)),
        "semicolon" => (";", ":", vk(0xBA)),
        "quote" => ("'", "\"", vk(0xDE)),
        "enter" => ("Enter", "Enter", vk(0x0D)),
        "comma" => (",", "<", vk(0xBC)),
        "period" => (".", ">", vk(0xBE)),
        "slash" => ("/", "?", vk(0xBF)),
        "space" => ("", "", vk(0x20)),
        "up" => ("↑", "↑", ext(0x26)),
        "down" => ("↓", "↓", ext(0x28)),
        "left" => ("←", "←", ext(0x25)),
        "right" => ("→", "→", ext(0x27)),
        "delete" => ("Del", "Del", ext(0x2E)),
        "home" => ("Home", "Home", ext(0x24)),
        "end" => ("End", "End", ext(0x23)),
        "pgup" => ("PgUp", "PgUp", ext(0x21)),
        "pgdn" => ("PgDn", "PgDn", ext(0x22)),
        "ime" => ("", "", Ime),
        "shift" => ("Shift", "Shift", Shift),
        "ctrl" => ("Ctrl", "Ctrl", Ctrl),
        "alt" => ("Alt", "Alt", Alt),
        "close" => ("Close", "Close", Close),
        _ => {
            // Single letters a–z.
            let b = name.as_bytes();
            if b.len() == 1 && b[0].is_ascii_lowercase() {
                const LOWER: [&str; 26] = [
                    "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s", "t", "u", "v", "w", "x",
                    "y", "z",
                ];
                const UPPER: [&str; 26] = [
                    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S", "T", "U", "V", "W", "X",
                    "Y", "Z",
                ];
                let i = (b[0] - b'a') as usize;
                return Some((LOWER[i], UPPER[i], vk(b[0].to_ascii_uppercase() as u16)));
            }
            return None;
        }
    })
}

/// `name[:width]` or `0xVK=Label[:width]`.
fn parse_token(tok: &str) -> Result<KeyDef, String> {
    let (body, width) = match tok.rsplit_once(':') {
        Some((b, w)) if w.parse::<f32>().is_ok() => (b, w.parse::<f32>().unwrap()),
        _ => (tok, 1.0),
    };
    if let Some(rest) = body.strip_prefix("0x") {
        let (hex, label) = rest.split_once('=').ok_or_else(|| format!("{tok}: expected 0xVK=Label"))?;
        let code = u16::from_str_radix(hex, 16).map_err(|_| format!("{tok}: bad key code"))?;
        let label = leak(label);
        return Ok(KeyDef { label, shifted: label, legend: "", legend_shifted: "", kind: KeyKind::Vk(code, false), width, custom: true });
    }
    let (label, shifted, kind) = catalog(body).ok_or_else(|| format!("unknown key name {body:?}"))?;
    Ok(KeyDef { label, shifted, legend: "", legend_shifted: "", kind, width, custom: false })
}

pub fn parse(code: &str, src: &str) -> Result<Layout, String> {
    let t = tinytoml::parse(src)?;
    let s = |path: &str| tinytoml::get(&t, path).and_then(Value::as_str).map(leak);
    let mut l = Layout {
        code: leak(code),
        name: s("name").unwrap_or(leak(code)),
        lcids: crate::i18n::lcids(tinytoml::get(&t, "lcid")),
        ime: match (s("ime.label"), s("ime.on"), s("ime.off")) {
            (Some(a), Some(b), Some(c)) => Some((a, b, c)),
            _ => None,
        },
        ime_vk: tinytoml::get(&t, "ime.fallback_vk").and_then(Value::as_int).unwrap_or(0) as u16,
        font: s("font"),
        legend_big_when_ime: tinytoml::get(&t, "ime.legends_big_when_on").and_then(Value::as_bool).unwrap_or(false),
        rows: Vec::new(),
    };
    let rows = tinytoml::get(&t, "rows").and_then(Value::as_array).ok_or("missing rows = [...]")?;
    for (i, row) in rows.iter().enumerate() {
        let row = row.as_str().ok_or_else(|| format!("row {} must be a string", i + 1))?;
        let keys: Result<Vec<KeyDef>, String> = row.split_whitespace().map(parse_token).collect();
        let keys = keys?;
        let w: f32 = keys.iter().map(|k| k.width).sum();
        if (w - ROW_UNITS).abs() > 0.01 {
            return Err(format!("row {} is {w} units wide, must be {ROW_UNITS}", i + 1));
        }
        l.rows.push(keys);
    }
    // Legends: [legends] q = ["ㅂ", "ㅃ"]
    if let Some(leg) = tinytoml::get(&t, "legends").and_then(Value::as_table) {
        for (name, v) in leg {
            let Some(arr) = v.as_array() else { continue };
            let Some(normal) = arr.first().and_then(Value::as_str) else { continue };
            let shifted = arr.get(1).and_then(Value::as_str).unwrap_or(normal);
            let (normal, shifted) = (leak(normal), leak(shifted));
            let Some((label, ..)) = catalog(name) else { return Err(format!("legend for unknown key {name:?}")) };
            for k in l.rows.iter_mut().flatten() {
                if !k.custom && k.label == label {
                    k.legend = normal;
                    k.legend_shifted = shifted;
                }
            }
        }
    }
    // The IME key takes its label from [ime].
    if let Some((label, ..)) = l.ime {
        for k in l.rows.iter_mut().flatten() {
            if k.kind == KeyKind::Ime {
                k.label = label;
                k.shifted = label;
            }
        }
    }
    Ok(l)
}

static LAYOUTS: OnceLock<Vec<Layout>> = OnceLock::new();

fn load(user_dir: Option<&Path>) -> Vec<Layout> {
    let mut out: Vec<Layout> = BUILTIN.iter().map(|(c, t)| parse(c, t).expect("built-in keyboard layout")).collect();
    for (code, txt) in user_files(user_dir) {
        match parse(&code, &txt) {
            Ok(l) => match out.iter().position(|q| q.code == l.code) {
                Some(i) => out[i] = l,
                None => out.push(l),
            },
            // A broken drop-in file must never take the keyboard down.
            Err(err) => eprintln!("keyboard layout {code}.toml ignored: {err}"),
        }
    }
    out
}

pub fn init(exe_dir: &Path) {
    let _ = LAYOUTS.set(load(Some(&exe_dir.join("i18n").join("keyboards"))));
}

fn layouts() -> &'static [Layout] {
    LAYOUTS.get_or_init(|| load(None))
}

/// Layout for an input language id; English (or the first one) otherwise.
pub fn for_lcid(lcid: u16) -> &'static Layout {
    let ls = layouts();
    ls.iter().find(|l| l.lcids.contains(&lcid)).or_else(|| ls.iter().find(|l| l.code == "en")).unwrap_or(&ls[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_layouts_load_and_are_15_units() {
        for code in ["ko", "ja", "en"] {
            assert!(layouts().iter().any(|l| l.code == code), "{code} missing");
        }
        let ko = for_lcid(0x12);
        let q = ko.rows.iter().flatten().find(|k| k.label == "q").unwrap();
        assert_eq!((q.legend, q.legend_shifted), ("ㅂ", "ㅃ"));
        assert!(ko.rows.iter().flatten().any(|k| k.kind == KeyKind::Ime && k.label == "한/영"));
        let ja = for_lcid(0x11);
        assert!(ja.rows.iter().flatten().any(|k| k.label == "変換" && k.kind == KeyKind::Vk(0x1C, false)));
        // Unknown input language falls back to English.
        assert_eq!(for_lcid(0x07).code, "en");
    }

    #[test]
    fn user_folder_adds_layouts_and_skips_broken_ones() {
        let dir = std::env::temp_dir().join("ufo_kbd_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let qwertz = r#"name = "Deutsch"
lcid = [0x07]
rows = [
  "esc grave 1 2 3 4 5 6 7 8 9 0 minus equal backspace",
  "tab:1.5 q w e r t z u i o p lbracket rbracket backslash:1.5",
  "caps:1.75 a s d f g h j k l semicolon quote enter:2.25",
  "shift:2.25 y x c v b n m comma period slash up delete:1.75",
  "ctrl:1.5 alt:1.5 space:7 left down right close:2",
]
"#;
        std::fs::write(dir.join("de.toml"), qwertz).unwrap();
        std::fs::write(dir.join("broken.toml"), "rows = [\"q w e\"]\n").unwrap();
        let ls = load(Some(&dir));
        let de = ls.iter().find(|l| l.code == "de").expect("added");
        assert_eq!(de.rows[1][6].label, "z");
        assert!(ls.iter().all(|l| l.code != "broken"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bad_files_are_rejected_with_a_reason() {
        assert!(parse("x", "rows = [\"q w e\"]").unwrap_err().contains("units"));
        assert!(parse("x", "rows = [\"nope:15\"]").unwrap_err().contains("unknown key"));
        assert!(parse("x", "name = \"no rows\"").unwrap_err().contains("rows"));
        assert!(parse("x", "rows = [\"q:15\"]\n[legends]\nnope = [\"x\"]").unwrap_err().contains("legend"));
        let ok = parse("x", "rows = [\"0x41=A:14 close\"]").unwrap();
        assert_eq!(ok.rows[0][0].kind, KeyKind::Vk(0x41, false));
    }
}
