//! `ufoverlay.ini` next to the exe: plain `key = value` lines, `#` comments.
//! Missing file or keys fall back to defaults. The in-VR settings panel writes
//! the whole file back with `save`.

use std::path::Path;

#[derive(Clone, Debug)]
pub struct Config {
    /// Panel refresh while nobody points at it.
    pub fps_min: u32,
    /// Panel refresh while pointed at / grabbed / 영상 mode.
    pub fps_max: u32,
    /// Seconds a panel stays at fps_max after the last interaction.
    pub boost_secs: f32,
    pub panel_alpha: f32,
    pub spawn_distance_m: f32,
    pub scroll_speed: f32,
    /// Meters per second at full stick while holding a panel.
    pub push_speed: f32,
    pub wrist: bool,
    pub wrist_width_m: f32,
    pub wrist_24h: bool,
    /// Ignore grip on the watch, so it can't be moved by accident.
    pub wrist_lock: bool,
    /// Wrist placement saved after you move it by hand (controller space).
    pub wrist_pose: Option<[f32; 12]>,
    /// UFO flies in and beams the watch onto your wrist at startup.
    pub boot_anim: bool,
    /// Keep the game from seeing trigger/grip/stick while you point at UFOverlay.
    pub block_input: bool,
    /// Pointer jitter filter: 0 off, 1 light, 2 strong.
    pub smoothing: u32,
    /// Watch on the right hand, pointer on the left.
    pub left_handed: bool,
    /// Relaunch elevated (via a scheduled task) so admin windows accept input.
    pub admin: bool,
    /// UI language pack code (`ko`, `ja`, `en`, …) or `auto` to follow Windows.
    pub language: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            fps_min: 15,
            fps_max: 72,
            boost_secs: 1.5,
            panel_alpha: 1.0,
            spawn_distance_m: 1.0,
            scroll_speed: 1.0,
            push_speed: 1.5,
            wrist: true,
            wrist_width_m: 0.085,
            wrist_24h: true,
            wrist_lock: false,
            wrist_pose: None,
            boot_anim: true,
            block_input: true,
            smoothing: 1,
            left_handed: false,
            admin: false,
            language: "auto".into(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Self {
        let mut c = Config::default();
        let Ok(txt) = std::fs::read_to_string(path) else {
            c.save(path);
            return c;
        };
        for line in txt.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            let f = || v.parse::<f32>().ok();
            let b = || matches!(v, "true" | "1" | "yes" | "on");
            match k {
                "fps_min" => c.fps_min = v.parse().unwrap_or(c.fps_min),
                "fps_max" => c.fps_max = v.parse().unwrap_or(c.fps_max),
                "boost_secs" => c.boost_secs = f().unwrap_or(c.boost_secs),
                "panel_alpha" => c.panel_alpha = f().unwrap_or(c.panel_alpha),
                "spawn_distance_m" => c.spawn_distance_m = f().unwrap_or(c.spawn_distance_m),
                "scroll_speed" => c.scroll_speed = f().unwrap_or(c.scroll_speed),
                "push_speed" => c.push_speed = f().unwrap_or(c.push_speed),
                "wrist" => c.wrist = b(),
                "wrist_width_m" => c.wrist_width_m = f().unwrap_or(c.wrist_width_m),
                "wrist_24h" => c.wrist_24h = b(),
                "wrist_lock" => c.wrist_lock = b(),
                "boot_anim" => c.boot_anim = b(),
                "block_input" => c.block_input = b(),
                "smoothing" => c.smoothing = v.parse().unwrap_or(c.smoothing),
                "left_handed" => c.left_handed = b(),
                "admin" => c.admin = b(),
                "language" => c.language = if v.is_empty() { "auto".into() } else { v.to_string() },
                "wrist_pose" => {
                    let p: Vec<f32> = v.split(',').filter_map(|s| s.trim().parse().ok()).collect();
                    if p.len() == 12 {
                        let mut m = [0.0; 12];
                        m.copy_from_slice(&p);
                        c.wrist_pose = Some(m);
                    }
                }
                _ => {}
            }
        }
        c.sanitize();
        c
    }

    /// Keep every value in a usable range (also used after settings edits).
    pub fn sanitize(&mut self) {
        self.fps_min = self.fps_min.clamp(1, 240);
        self.fps_max = self.fps_max.clamp(1, 240).max(self.fps_min);
        self.boost_secs = self.boost_secs.clamp(0.0, 30.0);
        self.panel_alpha = self.panel_alpha.clamp(0.05, 1.0);
        self.spawn_distance_m = self.spawn_distance_m.clamp(0.2, 10.0);
        self.scroll_speed = self.scroll_speed.clamp(0.05, 10.0);
        self.push_speed = self.push_speed.clamp(0.1, 10.0);
        self.wrist_width_m = self.wrist_width_m.clamp(0.03, 0.3);
        self.smoothing = self.smoothing.min(2);
    }

    pub fn save(&self, path: &Path) {
        let mut s = format!(
            "\
# UFOverlay settings. The in-VR settings panel (menu → ⚙) edits this file too.
fps_min = {}             # panel refresh when nobody is pointing at it
fps_max = {}             # refresh while pointing / grabbing / 영상 mode
boost_secs = {}          # stay at fps_max this long after interaction
panel_alpha = {}
spawn_distance_m = {}
scroll_speed = {}        # right stick scroll while pointing
push_speed = {}          # m/s, stick up/down while holding a panel
wrist = {}               # UFO watch on the back of the left hand (opens the menu)
wrist_width_m = {}
wrist_24h = {}
wrist_lock = {}          # true: the watch can't be grabbed / moved
boot_anim = {}           # UFO flies in and beams the watch on at startup
block_input = {}         # hide trigger/grip/stick from the game while pointing at UFOverlay
smoothing = {}           # pointer jitter filter: 0 off, 1 light, 2 strong
left_handed = {}         # watch on the right hand, pointer on the left
admin = {}               # relaunch elevated so admin windows accept clicks
language = {}            # auto, or a pack code from i18n/lang (ko, ja, en, …)
# Grab the watch with the right grip to move it; the spot is saved as wrist_pose.
",
            self.fps_min,
            self.fps_max,
            self.boost_secs,
            self.panel_alpha,
            self.spawn_distance_m,
            self.scroll_speed,
            self.push_speed,
            self.wrist,
            self.wrist_width_m,
            self.wrist_24h,
            self.wrist_lock,
            self.boot_anim,
            self.block_input,
            self.smoothing,
            self.left_handed,
            self.admin,
            self.language,
        );
        if let Some(m) = &self.wrist_pose {
            let vals: Vec<String> = m.iter().map(|v| format!("{v:.4}")).collect();
            s.push_str(&format!("wrist_pose = {}\n", vals.join(", ")));
        }
        let _ = std::fs::write(path, s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_then_load_roundtrips() {
        let path = std::env::temp_dir().join("ufo_cfg_test.ini");
        let mut c = Config { fps_min: 20, fps_max: 90, boot_anim: false, wrist_width_m: 0.1, ..Config::default() };
        c.wrist_pose = Some([1., 0., 0., 0.1, 0., 1., 0., 0.2, 0., 0., 1., 0.3]);
        c.save(&path);
        let d = Config::load(&path);
        assert_eq!((d.fps_min, d.fps_max, d.boot_anim), (20, 90, false));
        assert!((d.wrist_width_m - 0.1).abs() < 1e-6);
        assert_eq!(d.wrist_pose, c.wrist_pose);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn sanitize_keeps_max_above_min() {
        let mut c = Config { fps_min: 60, fps_max: 30, ..Config::default() };
        c.sanitize();
        assert!(c.fps_max >= c.fps_min);
    }
}
