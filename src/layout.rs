//! Panel layout persistence: `layout.txt` next to the exe, one panel per line,
//! tab-separated. Windows are re-found by (exe, class) and preferably title,
//! because HWNDs don't survive a restart; monitors by their desktop rectangle.

use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub enum SavedSrc {
    Monitor { rect: [i32; 4] },
    Window { exe: String, class: String, title: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Saved {
    pub src: SavedSrc,
    pub width: f32,
    pub live: bool,
    /// Input locked (view only).
    pub locked: bool,
    pub world: [f32; 12],
    /// (hand, panel-in-controller transform) for panels carried on a hand.
    pub attached: Option<(usize, [f32; 12])>,
}

fn floats(m: &[f32; 12]) -> String {
    m.iter().map(|v| format!("{v:.5}")).collect::<Vec<_>>().join(",")
}

fn parse12(s: &str) -> Option<[f32; 12]> {
    let v: Vec<f32> = s.split(',').filter_map(|x| x.trim().parse().ok()).collect();
    (v.len() == 12).then(|| {
        let mut m = [0.0; 12];
        m.copy_from_slice(&v);
        m
    })
}

fn clean(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

pub fn encode(panels: &[Saved]) -> String {
    let mut out = String::from("# UFOverlay panel layout (written automatically)\n");
    for p in panels {
        let attach = match &p.attached {
            Some((h, m)) => format!("{h}:{}", floats(m)),
            None => "-".into(),
        };
        // flags: bit 0 = video (live), bit 1 = input locked. Old files have 0/1.
        let flags = p.live as u8 | (p.locked as u8) << 1;
        let common = format!("{:.4}\t{}\t{}\t{}", p.width, flags, floats(&p.world), attach);
        match &p.src {
            SavedSrc::Monitor { rect } => {
                out.push_str(&format!("monitor\t{},{},{},{}\t{common}\n", rect[0], rect[1], rect[2], rect[3]));
            }
            SavedSrc::Window { exe, class, title } => {
                out.push_str(&format!("window\t{}\t{}\t{common}\t{}\n", clean(exe), clean(class), clean(title)));
            }
        }
    }
    out
}

pub fn decode(txt: &str) -> Vec<Saved> {
    let mut out = Vec::new();
    for line in txt.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        let (src, rest) = match f.first().copied() {
            Some("monitor") if f.len() >= 6 => {
                let r: Vec<i32> = f[1].split(',').filter_map(|x| x.parse().ok()).collect();
                if r.len() != 4 {
                    continue;
                }
                (SavedSrc::Monitor { rect: [r[0], r[1], r[2], r[3]] }, &f[2..6])
            }
            Some("window") if f.len() >= 8 => {
                (SavedSrc::Window { exe: f[1].into(), class: f[2].into(), title: f[7..].join(" ") }, &f[3..7])
            }
            _ => continue,
        };
        let (Ok(width), Some(world)) = (rest[0].parse::<f32>(), parse12(rest[2])) else { continue };
        let attached = rest[3].split_once(':').and_then(|(h, m)| Some((h.parse::<usize>().ok()?.min(1), parse12(m)?)));
        let flags: u8 = rest[1].parse().unwrap_or(0);
        out.push(Saved { src, width, live: flags & 1 != 0, locked: flags & 2 != 0, world, attached });
    }
    out
}

pub fn save(path: &Path, panels: &[Saved]) {
    let _ = std::fs::write(path, encode(panels));
}

pub fn load(path: &Path) -> Vec<Saved> {
    std::fs::read_to_string(path).map(|t| decode(&t)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_monitor_and_window() {
        let m = [1., 0., 0., 0.5, 0., 1., 0., 1.4, 0., 0., 1., -1.];
        let panels = vec![
            Saved { src: SavedSrc::Monitor { rect: [0, 0, 2560, 1440] }, width: 1.2, live: false, locked: true, world: m, attached: None },
            Saved {
                src: SavedSrc::Window { exe: "Discord.exe".into(), class: "Chrome_WidgetWin_1".into(), title: "#일반\t| 서버".into() },
                width: 0.6,
                live: true,
                locked: false,
                world: m,
                attached: Some((0, m)),
            },
        ];
        let back = decode(&encode(&panels));
        assert_eq!(back.len(), 2);
        assert_eq!(back[0], panels[0]);
        assert_eq!(back[1].live, true);
        assert_eq!(back[1].attached.map(|a| a.0), Some(0));
        // Tabs in titles are flattened, not allowed to break the format.
        assert_eq!(
            back[1].src,
            SavedSrc::Window { exe: "Discord.exe".into(), class: "Chrome_WidgetWin_1".into(), title: "#일반 | 서버".into() }
        );
    }

    #[test]
    fn garbage_lines_are_skipped() {
        assert!(decode("monitor\tx\n\nwindow\ta\n# c\n").is_empty());
    }
}
