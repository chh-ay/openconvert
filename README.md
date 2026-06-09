# OpenConvert

An interactive video editor built in Rust with [egui](https://github.com/emilk/egui): a multi-track timeline with draggable clips, trim/split, zoom/scroll, image and audio clips, layered live preview, and in-process libav export.

## Prerequisites

- A recent stable **Rust** toolchain (edition 2021).
- **FFmpeg/libav development libraries** (`libavcodec`, `libavformat`, `libavutil`, `libswscale`, `libswresample`) plus `pkg-config` and `libclang` — the default development build dynamically links libav.
- **Desktop libraries**: ALSA for audio playback, and Wayland/X11 + OpenGL for the window.

Example (Debian/Ubuntu); Arch equivalents are `ffmpeg clang pkgconf alsa-lib`:

```sh
sudo apt install build-essential pkg-config libclang-dev \
  libavcodec-dev libavformat-dev libavutil-dev libswscale-dev libswresample-dev \
  libasound2-dev
```

## Build & run

Development builds dynamically link system libav for fast iteration:

```sh
cargo run -p openconvert-app --release
```

For a release build that embeds libav (no system FFmpeg needed):

```sh
cargo build -p openconvert-app --release --features static-ffmpeg
```

See [`packaging/README.md`](packaging/README.md) for the per-OS build and
packaging strategy (Arch dynamic distro package, macOS binary, Windows single
`.exe`) and the `.github/workflows/release.yml` CI matrix.

## Static release notes

The static release build embeds FFmpeg/libav, so users do not need FFmpeg installed and no `ffmpeg` executable is shipped. External codec libraries (x264, x265, vpx, opus, mp3lame) are folded in only when static `.a` archives are selected; the Windows release job forces that mode and fails if the final `.exe` still depends on `/mingw64/bin/*.dll`.

- **Requires `nasm` and a C toolchain** — it compiles FFmpeg from source, so the first build is slow and the binary grows.
- The GPL codec set (x264/x265) makes the resulting binary GPL-licensed.
- **Cross-platform:** build natively per OS; cross-compiling static FFmpeg from one host is impractical.
- Export/conversion runs in-process too; with this feature there is no runtime `ffmpeg` executable or vendored binary.

## Develop

```sh
cargo test --workspace                                 # unit + integration tests
cargo clippy --workspace --all-targets -- -D warnings  # lints (warnings are errors)
cargo fmt --all                                        # format
```

## Workspace layout

| Crate               | Responsibility                                                                        |
| ------------------- | ------------------------------------------------------------------------------------- |
| `openconvert-core`  | Timeline/clip model, tracks, undo/redo — pure logic, no I/O.                          |
| `openconvert-media` | In-process libav decode/probe/render, CPU layer compositor, export settings.          |
| `openconvert-app`   | egui UI: timeline editor, preview/playback, transport, export.                        |

## Notes

- Preview and thumbnails decode in-process via libav (no per-frame process spawn); scrubbing reuses a persistent decoder so seeking within a clip avoids reopening the source.
- Export and conversion render in-process via libav; no runtime `ffmpeg` process or vendored binary is required.
- The release profile optimizes for size (`opt-level = "z"`, thin LTO, `panic = "abort"`, stripped).
