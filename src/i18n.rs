//! Language packs: `i18n/lang/<code>.toml` (standard TOML, read by tinytoml).
//!
//! The packs in the repo are compiled in; a file with the same name in
//! `<exe dir>/i18n/lang/` overrides it, and a new name adds a language — no
//! rebuild needed. Code asks for stable ids (`tr("menu.refresh")`); a missing
//! entry falls back to English, then to the id itself.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

use crate::tinytoml::{self, Table, Value};

pub struct Pack {
    pub code: &'static str,
    pub name: &'static str,
    pub font: &'static str,
    pub lcids: Vec<u16>,
    map: HashMap<&'static str, &'static str>,
}

const BUILTIN: &[(&str, &str)] = &[
    ("ko", include_str!("../i18n/lang/ko.toml")),
    ("ja", include_str!("../i18n/lang/ja.toml")),
    ("en", include_str!("../i18n/lang/en.toml")),
];

static PACKS: OnceLock<Vec<Pack>> = OnceLock::new();
static CURRENT: AtomicUsize = AtomicUsize::new(0);

/// Strings live for the whole run; leaking the few KB of packs once keeps
/// `tr` returning plain `&'static str` with no locking.
fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

pub fn lcids(v: Option<&Value>) -> Vec<u16> {
    v.and_then(Value::as_array).unwrap_or_default().iter().filter_map(|x| x.as_int()).map(|x| x as u16).collect()
}

/// Flatten every string under every table into "section.key".
fn flatten(prefix: &str, t: &Table, out: &mut HashMap<&'static str, &'static str>) {
    for (k, v) in t {
        let id = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
        match v {
            Value::Str(s) => {
                out.insert(leak(&id), leak(s));
            }
            Value::Table(sub) => flatten(&id, sub, out),
            _ => {}
        }
    }
}

pub fn parse(code: &str, src: &str) -> Result<Pack, String> {
    let t = tinytoml::parse(src)?;
    let s = |path: &str| tinytoml::get(&t, path).and_then(Value::as_str).map(leak);
    let mut map = HashMap::new();
    for (k, v) in &t {
        if let (false, Value::Table(sub)) = (k == "meta", v) {
            flatten(k, sub, &mut map);
        }
    }
    let code = leak(code);
    Ok(Pack {
        code,
        name: s("meta.name").unwrap_or(code),
        font: s("meta.font").unwrap_or("Segoe UI"),
        lcids: lcids(tinytoml::get(&t, "meta.lcid")),
        map,
    })
}

fn load(user_dir: Option<&Path>) -> Vec<Pack> {
    let mut packs: Vec<Pack> = BUILTIN.iter().map(|(c, t)| parse(c, t).expect("built-in language pack")).collect();
    for (code, txt) in crate::kbd::user_files(user_dir) {
        match parse(&code, &txt) {
            Ok(p) => match packs.iter().position(|q| q.code == p.code) {
                Some(i) => packs[i] = p,
                None => packs.push(p),
            },
            Err(e) => eprintln!("language pack {code}.toml ignored: {e}"),
        }
    }
    packs
}

/// Load packs (built-in + `<exe dir>/i18n/lang`). Call once at startup;
/// without it only the built-in packs are used.
pub fn init(exe_dir: &Path) {
    let _ = PACKS.set(load(Some(&exe_dir.join("i18n").join("lang"))));
}

fn packs() -> &'static [Pack] {
    PACKS.get_or_init(|| load(None))
}

/// Available languages as (code, name), in load order.
pub fn languages() -> Vec<(&'static str, &'static str)> {
    packs().iter().map(|p| (p.code, p.name)).collect()
}

/// Windows UI language id (primary language only).
fn system_lcid() -> u16 {
    unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() & 0x3FF }
}

/// `code` = a pack code, or "auto" to follow Windows (falls back to English).
pub fn set(code: &str) {
    let ps = packs();
    let en = ps.iter().position(|p| p.code == "en");
    let idx = if code == "auto" || code.is_empty() {
        let id = system_lcid();
        ps.iter().position(|p| p.lcids.contains(&id)).or(en)
    } else {
        ps.iter().position(|p| p.code == code).or(en)
    };
    CURRENT.store(idx.unwrap_or(0), Ordering::Relaxed);
}

pub fn current() -> &'static Pack {
    let ps = packs();
    &ps[CURRENT.load(Ordering::Relaxed).min(ps.len() - 1)]
}

pub fn code() -> &'static str {
    current().code
}

pub fn font() -> &'static str {
    current().font
}

/// Text for a stable id like "menu.refresh": current pack → English → id.
pub fn tr(id: &'static str) -> &'static str {
    if let Some(s) = current().map.get(id) {
        return s;
    }
    packs().iter().find(|p| p.code == "en").and_then(|p| p.map.get(id).copied()).unwrap_or(id)
}

/// "1.5 s" / "1.5초" / "1.5秒" from the pack's `settings.seconds` template.
pub fn seconds(v: f32) -> String {
    tr("settings.seconds").replace("{v}", &format!("{v:.1}"))
}

#[cfg(test)]
pub fn keys_of(code: &str) -> Vec<&'static str> {
    packs().iter().find(|p| p.code == code).map(|p| p.map.keys().copied().collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_packs_parse_and_switch() {
        let codes: Vec<_> = languages().iter().map(|l| l.0).collect();
        assert!(codes.contains(&"ko") && codes.contains(&"ja") && codes.contains(&"en"));
        set("ja");
        assert_eq!((code(), tr("menu.refresh"), font()), ("ja", "更新", "Yu Gothic UI"));
        assert_eq!(tr("no.such.id"), "no.such.id");
        set("en");
        assert_eq!(seconds(1.5), "1.5 s");
        set("ko");
        assert_eq!(seconds(1.5), "1.5초");
        set("xx"); // unknown → English
        assert_eq!(code(), "en");
        set("ko");
    }

    #[test]
    fn user_folder_adds_and_overrides_packs() {
        let dir = std::env::temp_dir().join("ufo_lang_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("fr.toml"), "[meta]\nname = \"Français\"\n[menu]\nrefresh = \"Actualiser\"\n").unwrap();
        std::fs::write(dir.join("en.toml"), "[meta]\nname = \"English (custom)\"\n[menu]\nrefresh = \"Reload\"\n").unwrap();
        std::fs::write(dir.join("junk.toml"), "this is = = not toml\n").unwrap();
        std::fs::write(dir.join("notes.txt"), "ignored\n").unwrap();
        let packs = load(Some(&dir));
        let fr = packs.iter().find(|p| p.code == "fr").expect("added");
        assert_eq!((fr.name, fr.map.get("menu.refresh").copied()), ("Français", Some("Actualiser")));
        let en = packs.iter().find(|p| p.code == "en").unwrap();
        assert_eq!(en.map.get("menu.refresh").copied(), Some("Reload"));
        assert!(packs.iter().all(|p| p.code != "junk" && p.code != "notes"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every id of the reference pack (English) must exist in every built-in pack.
    #[test]
    fn every_pack_translates_every_key() {
        let reference = keys_of("en");
        assert!(reference.len() > 30);
        for (code, _) in languages() {
            let have = keys_of(code);
            let missing: Vec<_> = reference.iter().filter(|k| !have.contains(k)).collect();
            let extra: Vec<_> = have.iter().filter(|k| !reference.contains(k)).collect();
            assert!(missing.is_empty(), "{code}.toml is missing {missing:?}");
            assert!(extra.is_empty(), "{code}.toml has ids en.toml doesn't: {extra:?}");
        }
    }

    /// Every `tr("…")` id in the code must exist in en.toml.
    #[test]
    fn code_uses_only_declared_ids() {
        let reference = keys_of("en");
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            if e.file_name() == "i18n.rs" {
                continue; // its docs and tests use example ids
            }
            let txt = std::fs::read_to_string(e.path()).unwrap_or_default();
            for part in txt.split("tr(\"").skip(1) {
                let id = &part[..part.find('"').unwrap()];
                assert!(reference.contains(&id), "{:?} uses unknown id {id:?}", e.file_name());
            }
        }
        for it in crate::settings::items() {
            for id in [it.section, it.label] {
                assert!(reference.contains(&id), "settings id {id:?} not in en.toml");
            }
        }
    }
}
