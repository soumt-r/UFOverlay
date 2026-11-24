//! What the in-VR settings panel shows and how each row edits the Config.
//! Pure data + logic (no drawing, no VR) so it is easy to test.

use crate::config::Config;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Id {
    FpsMin,
    FpsMax,
    Boost,
    Alpha,
    Spawn,
    Scroll,
    Push,
    WristSize,
    Wrist24h,
    BootAnim,
    WristReset,
    Autostart,
    BlockInput,
    Smoothing,
    LeftHanded,
    Admin,
    Language,
}

#[derive(Clone, Copy, Debug)]
pub enum Kind {
    Num { min: f32, max: f32, step: f32 },
    Toggle,
    Action(&'static str),
}

pub struct Item {
    pub id: Id,
    pub section: &'static str,
    pub label: &'static str,
    pub kind: Kind,
}

pub fn items() -> Vec<Item> {
    use Id::*;
    use Kind::*;
    let it = |id, section, label, kind| Item { id, section, label, kind };
    vec![
        // max is replaced at runtime by the number of installed language packs.
        it(Language, "settings.display", "settings.language", Num { min: 0.0, max: 99.0, step: 1.0 }),
        it(FpsMin, "settings.display", "settings.fps_idle", Num { min: 5.0, max: 60.0, step: 5.0 }),
        it(FpsMax, "settings.display", "settings.fps_pointing", Num { min: 30.0, max: 144.0, step: 6.0 }),
        it(Boost, "settings.display", "settings.boost", Num { min: 0.5, max: 5.0, step: 0.5 }),
        it(Alpha, "settings.display", "settings.opacity", Num { min: 0.3, max: 1.0, step: 0.1 }),
        it(Spawn, "settings.display", "settings.spawn_distance", Num { min: 0.5, max: 3.0, step: 0.25 }),
        it(Scroll, "settings.controls", "settings.scroll_speed", Num { min: 0.25, max: 3.0, step: 0.25 }),
        it(Push, "settings.controls", "settings.push_speed", Num { min: 0.5, max: 4.0, step: 0.5 }),
        it(Smoothing, "settings.controls", "settings.smoothing", Num { min: 0.0, max: 2.0, step: 1.0 }),
        it(BlockInput, "settings.controls", "settings.block_input", Toggle),
        it(LeftHanded, "settings.controls", "settings.left_handed", Toggle),
        it(WristSize, "settings.wrist", "settings.watch_size", Num { min: 0.05, max: 0.15, step: 0.01 }),
        it(Wrist24h, "settings.wrist", "settings.clock_24h", Toggle),
        it(BootAnim, "settings.wrist", "settings.boot_anim", Toggle),
        it(WristReset, "settings.wrist", "settings.watch_position", Action("settings.reset")),
        it(Autostart, "settings.startup", "settings.autostart", Toggle),
        it(Admin, "settings.startup", "settings.admin", Toggle),
    ]
}

/// 0 = auto, k = k-th installed language pack.
fn language_index(code: &str) -> usize {
    crate::i18n::languages().iter().position(|l| l.0 == code).map(|i| i + 1).unwrap_or(0)
}

/// Last selectable value of the language row.
pub fn language_max() -> f32 {
    crate::i18n::languages().len() as f32
}

/// Current numeric value of a row.
pub fn num(cfg: &Config, id: Id) -> f32 {
    match id {
        Id::FpsMin => cfg.fps_min as f32,
        Id::FpsMax => cfg.fps_max as f32,
        Id::Boost => cfg.boost_secs,
        Id::Alpha => cfg.panel_alpha,
        Id::Spawn => cfg.spawn_distance_m,
        Id::Scroll => cfg.scroll_speed,
        Id::Push => cfg.push_speed,
        Id::WristSize => cfg.wrist_width_m,
        Id::Smoothing => cfg.smoothing as f32,
        Id::Language => language_index(&cfg.language) as f32,
        _ => 0.0,
    }
}

/// Human-friendly text for a numeric row.
pub fn format(id: Id, v: f32) -> String {
    match id {
        Id::FpsMin | Id::FpsMax => format!("{} fps", v.round() as i32),
        Id::Boost => crate::i18n::seconds(v),
        Id::Smoothing => crate::i18n::tr(["settings.off", "settings.light", "settings.strong"][(v.round() as usize).min(2)]).to_string(),
        Id::Language => match v.round() as usize {
            0 => crate::i18n::tr("settings.auto").to_string(),
            k => crate::i18n::languages().get(k - 1).map(|l| l.1.to_string()).unwrap_or_default(),
        },
        Id::Alpha => format!("{}%", (v * 100.0).round() as i32),
        Id::Spawn => format!("{v:.2} m"),
        Id::Scroll | Id::Push => format!("×{v:.2}").trim_end_matches('0').trim_end_matches('.').to_string(),
        Id::WristSize => {
            let cm = v * 100.0;
            if (cm - cm.round()).abs() < 0.05 {
                format!("{} cm", cm.round() as i32)
            } else {
                format!("{cm:.1} cm")
            }
        }
        _ => String::new(),
    }
}

/// Step a numeric row by `dir` (−1 / +1), snapping to the step grid.
pub fn step(cfg: &mut Config, id: Id, dir: i32) {
    let Some(Kind::Num { min, mut max, step }) = items().into_iter().find(|i| i.id == id).map(|i| i.kind) else { return };
    if id == Id::Language {
        max = language_max();
    }
    let cur = num(cfg, id);
    let snapped = ((cur - min) / step).round() * step + min;
    let next = if (snapped - cur).abs() > step * 0.01 && dir != 0 {
        // Off-grid (hand-edited ini): first press lands on the grid in that direction.
        if dir > 0 {
            (((cur - min) / step).ceil() * step + min).max(cur)
        } else {
            ((cur - min) / step).floor() * step + min
        }
    } else {
        snapped + dir as f32 * step
    };
    let v = next.clamp(min, max);
    match id {
        Id::FpsMin => cfg.fps_min = v.round() as u32,
        Id::FpsMax => cfg.fps_max = v.round() as u32,
        Id::Boost => cfg.boost_secs = v,
        Id::Alpha => cfg.panel_alpha = v,
        Id::Spawn => cfg.spawn_distance_m = v,
        Id::Scroll => cfg.scroll_speed = v,
        Id::Push => cfg.push_speed = v,
        Id::WristSize => cfg.wrist_width_m = v,
        Id::Smoothing => cfg.smoothing = v.round() as u32,
        Id::Language => {
            let langs = crate::i18n::languages();
            let k = (v.round() as usize).min(langs.len());
            cfg.language = if k == 0 { "auto".into() } else { langs[k - 1].0.to_string() };
        }
        _ => {}
    }
    // Keep max ≥ min by pushing the *other* one.
    if cfg.fps_max < cfg.fps_min {
        if id == Id::FpsMin {
            cfg.fps_max = cfg.fps_min
        } else {
            cfg.fps_min = cfg.fps_max
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_stay_on_grid_and_in_range() {
        let mut c = Config::default();
        step(&mut c, Id::FpsMax, 1);
        assert_eq!(c.fps_max, 78);
        for _ in 0..50 {
            step(&mut c, Id::FpsMax, 1);
        }
        assert_eq!(c.fps_max, 144);
        step(&mut c, Id::Alpha, -1);
        assert!((c.panel_alpha - 0.9).abs() < 1e-4);
    }

    #[test]
    fn min_pushes_max_up() {
        let mut c = Config { fps_min: 30, fps_max: 30, ..Config::default() };
        step(&mut c, Id::FpsMin, 1);
        assert_eq!((c.fps_min, c.fps_max), (35, 35));
    }

    #[test]
    fn off_grid_value_snaps_first() {
        let mut c = Config { scroll_speed: 1.1, ..Config::default() };
        step(&mut c, Id::Scroll, 1);
        assert!((c.scroll_speed - 1.25).abs() < 1e-4);
    }

    #[test]
    fn formats_read_naturally() {
        assert_eq!(format(Id::Alpha, 0.8), "80%");
        assert_eq!(format(Id::Scroll, 1.0), "×1");
        assert_eq!(format(Id::Scroll, 1.25), "×1.25");
        assert_eq!(format(Id::WristSize, 0.085), "8.5 cm");
        assert_eq!(format(Id::WristSize, 0.09), "9 cm");
    }
}
