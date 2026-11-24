# Languages & keyboards / 언어·키보드 추가하기

All files here are **standard TOML** — any editor highlights and checks them,
and translation platforms (Crowdin, Weblate, …) can import them.
A file's **name is its code**: `fr.toml` → `language = "fr"` in `ufoverlay.ini`.

- **Try it without rebuilding:** put the file in `i18n/lang/` or
  `i18n/keyboards/` **next to `ufoverlay.exe`** and restart. Same name as a
  built-in file → replaces it; a new name → adds it.
- **Ship it with UFOverlay:** add the file here, add one line to `BUILTIN` in
  `src/i18n.rs` or `src/kbd.rs`, then `cargo test`.

## Adding a language — `lang/<code>.toml`

Copy `en.toml` (the reference, with a note on every line) and translate the
values:

```toml
[meta]
name = "Français"      # shown in Settings → Language, in its own language
font = "Segoe UI"      # a Windows font that covers your script
lcid = [0x0c]          # Windows UI language id(s); "auto" picks this pack

[menu]
refresh = "Actualiser"

[settings]
seconds = "{v} s"      # {v} is replaced by the number
```

Missing entries fall back to English, so a partial pack already works.
`cargo test` fails if a built-in pack misses an id (or has one `en.toml`
doesn't), naming the id.

## Adding a keyboard layout — `keyboards/<code>.toml`

The VR keyboard follows the **input language of the focused window**: the
first layout whose `lcid` matches is shown, otherwise `en`.

```toml
name = "Deutsch (QWERTZ)"
lcid = [0x07]
font = "Segoe UI"          # for legends and custom labels (optional)

# 5 rows, each exactly 15 units wide. Token: name[:width] or 0xVK=Label[:width]
rows = [
  "esc grave 1 2 3 4 5 6 7 8 9 0 minus equal backspace",
  "tab:1.5 q w e r t z u i o p lbracket rbracket backslash:1.5",
  "caps:1.75 a s d f g h j k l semicolon quote enter:2.25",
  "shift:2.25 y x c v b n m comma period slash up delete:1.75",
  "ctrl:1.5 alt:1.5 space:7 left down right close:2",
]

[ime]                       # optional: an IME toggle key (한/영, 半/全 …), use `ime` in a row
label = "한/영"             # small, on the key
on = "한"                   # big, while native input is on
off = "영"                  # big, while it is off
fallback_vk = 0x15          # sent if the IME can't be switched directly
legends_big_when_on = true  # legends big / Latin small while native input is on

[legends]                   # optional second character(s): [normal, with Shift]
q = ["ㅂ", "ㅃ"]
```

Keys send Windows virtual-key codes, so the system IME / layout turns them into
text exactly like a physical keyboard. A broken drop-in file is skipped (and
logged); it never breaks the keyboard.

### Key names

| name | key | name | key |
|---|---|---|---|
| `a` … `z`, `0` … `9` | letters, digits | `space` | Space |
| `esc` `tab` `caps` `enter` `backspace` `delete` | | `shift` `ctrl` `alt` | sticky modifiers |
| `grave` `minus` `equal` | `` ` `` `-` `=` | `lbracket` `rbracket` `backslash` | `[` `]` `\` |
| `semicolon` `quote` | `;` `'` | `comma` `period` `slash` | `,` `.` `/` |
| `up` `down` `left` `right` | arrows | `home` `end` `pgup` `pgdn` | navigation |
| `ime` | IME toggle (from `[ime]`) | `close` | closes the keyboard |

Anything else: `0xVK=Label[:width]` with a
[virtual-key code](https://learn.microsoft.com/windows/win32/inputdev/virtual-key-codes),
e.g. `0x1C=変換:1.25`.

## TOML features you can use

UFOverlay reads these files with a small built-in reader (a full TOML library
would add ~214 KB to the exe). It supports everything above: comments,
`[tables]`, dotted keys, `"strings"` with escapes, `'literal strings'`,
integers (`0x…` too), floats, booleans and (multi-line) arrays.
**Not supported:** multi-line strings (`"""`), inline tables (`{ … }`), arrays
of tables (`[[…]]`), dates. `cargo test` checks every shipped file against
the real `toml` parser, so built-in files can't drift from standard TOML.
