# GoldLaceRust

Clean-room Rust reconstruction of the classic **Gold Lace 2.2** procedural screensaver.

This implementation is based on static analysis notes summarized in
[`gold-lace-cleanroom-spec.md`](gold-lace-cleanroom-spec.md). The original packed
`.scr` / `.exe` files were not executed while producing the reconstruction.

## Features

- Native SDL2 windowing with Wayland-friendly fullscreen support.
- OpenGL/GPU renderer using a two-pass shader pipeline:
  - pass 1 renders the procedural scalar field into an `R32F` float texture
  - pass 2 applies palette lookup and edge-aware antialiasing
- All 46 recovered original palettes embedded from repo-local `palettes.json`.
- Native drawable-size rendering for resize, fullscreen, and Hi-DPI displays.
- Pattern history for moving backward and forward through generated patterns.

## Controls

| Key | Action |
|---|---|
| `Space` | Next pattern, or generate a new one at the end of history |
| `Shift` + `Space` | Previous pattern |
| `N` | Force-generate a new pattern branch |
| `[` / `]` | Previous / next palette |
| `P` | Pause/resume palette scrolling |
| `F` | Toggle desktop fullscreen |
| `Q` or `Esc` | Quit |

## Running on NixOS

Use the provided shell so SDL2 and `pkg-config` are available:

```bash
nix-shell
cargo run --release
```

The shell defaults to SDL's Wayland backend:

```bash
SDL_VIDEODRIVER=wayland
```

If your compositor/driver behaves oddly, try X11 instead:

```bash
SDL_VIDEODRIVER=x11 cargo run --release
```

## Building and testing

```bash
nix-shell --run 'cargo fmt && cargo test && cargo build --release'
```

## Notes

The current renderer is visually faithful but not bit-exact. It keeps some older
CPU-rendering code around as a reference while the GPU path settles. The shader
layout is intentionally close to a future WebGL2/Shadertoy-style port: scalar
field first, palette/color/AA second.

Palette data lives in [`palettes.json`](palettes.json), converted from the
recovered `palettes.plt` format so this repo is self-contained.

## Licensing

- Rust source code: MIT, see [`LICENSE-MIT`](LICENSE-MIT).
- Documentation/spec text: CC0 1.0, see [`LICENSE-CC0`](LICENSE-CC0).
- `palettes.json`: original-derived compatibility data with unknown copyright
  status, see [`PALETTES-NOTICE.md`](PALETTES-NOTICE.md).
