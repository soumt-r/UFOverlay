//! A soft key "tock" for the VR keyboard, synthesized once at startup (no
//! sound files) and played with PlaySound from memory. ~2.5 KB of audio.

use std::sync::OnceLock;

use windows::core::PCWSTR;
use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

const RATE: u32 = 22_050;

/// 16-bit mono WAV: a short bright tick on top of a low, quickly fading thump.
fn synth_click() -> Vec<u8> {
    let n = (RATE as f32 * 0.045) as usize;
    let mut seed = 0x1234_5678u32;
    let mut pcm: Vec<i16> = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / RATE as f32;
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = (seed >> 9) as f32 / (1u32 << 23) as f32 * 2.0 - 1.0;
        let tick = noise * (-t / 0.0025).exp() * 0.30;
        let thump = (std::f32::consts::TAU * 170.0 * t).sin() * (-t / 0.011).exp() * 0.42;
        let ramp = (t / 0.0008).min(1.0); // no click-in pop
        pcm.push(((tick + thump) * ramp * 0.55 * i16::MAX as f32) as i16);
    }
    let data_len = (pcm.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + data_len as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data_len).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&RATE.to_le_bytes());
    w.extend_from_slice(&(RATE * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        w.extend_from_slice(&s.to_le_bytes());
    }
    w
}

static CLICK: OnceLock<Vec<u8>> = OnceLock::new();

/// Play the key sound (asynchronously; a new click cuts the previous one).
pub fn click() {
    let wav = CLICK.get_or_init(synth_click);
    unsafe {
        let _ = PlaySoundW(PCWSTR(wav.as_ptr() as *const u16), None, SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn click_is_a_short_valid_wav() {
        let w = super::synth_click();
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(&w[8..12], b"WAVE");
        assert!(w.len() < 4096, "{} bytes", w.len());
        let peak = w[44..].chunks(2).map(|c| i16::from_le_bytes([c[0], c[1]]).unsigned_abs()).max().unwrap();
        assert!(peak > 3000 && peak < i16::MAX as u16, "peak {peak}");
    }
}
