# Contributing

Thanks for helping! A few things keep UFOverlay small and fast.

## The one rule: stay light

Being light is UFOverlay's whole point. Every change should keep these true:

- Nothing visible → (almost) nothing running. New work should be event-driven
  or tied to something the user is looking at.
- Pixels stay on the GPU. No per-frame CPU copies of captured images.
- Static UI is redrawn only when it changes.
- New dependencies need a good reason; note the release-exe size before/after
  in the PR (`cargo build --release`, check `target/release/ufoverlay.exe`).

For changes to the loop, capture or input, include before/after CPU numbers
(e.g. `Get-Process ufoverlay` → `TotalProcessorTime` over ~15 s, idle and
with one panel open).

## Before opening a PR

```powershell
cargo fmt
cargo clippy
cargo test
```

CI runs the same on Windows.

## Translations and keyboard layouts

No Rust needed — see [i18n/README.md](i18n/README.md). `cargo test` checks that
every built-in pack has every id and that every layout row is 15 units.

## Testing in VR

Most logic has unit tests, but anything about feel (pointer, grabbing, the
watch position, keyboard) needs a headset. Please say which headset /
controllers / streaming setup you tested with.
