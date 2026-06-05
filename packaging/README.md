# Packaging

Targets: Arch Linux, macOS, Windows. Each OS builds natively — cross-compiling a
static FFmpeg from one host is impractical. `.github/workflows/release.yml`
automates all three.

## Two build modes

- **Dynamic (default):** `cargo build -p openconvert-app --release` links the
  system libav. Smallest binary, fastest build, but the target machine must
  provide FFmpeg + codec libraries. Best for a distro package (e.g. Arch).
- **Self-contained:** add `--features static-ffmpeg` to statically link FFmpeg
  and the GPL codec set into the binary. Larger and slower to build (compiles
  FFmpeg from source; needs `nasm` + a C toolchain), but runs with no system
  libav. Best for macOS/Windows where users have no libav.

## Arch Linux

Use the dynamic build and depend on distro libraries — do not bundle a
self-contained ELF. A `PKGBUILD` is in `packaging/arch/`:

```sh
cd packaging/arch && makepkg -si
```

It depends on `ffmpeg`, `alsa-lib`, the X11/Wayland/GL libraries `eframe` needs,
`gcc-libs`, and `glibc`, plus one dialog provider (`zenity` or `kdialog`).

## macOS

Users have no libav, so build self-contained:

```sh
brew install nasm pkg-config x264 x265 libvpx opus lame
cargo build -p openconvert-app --release --locked --features static-ffmpeg
```

The codec libraries (x264/x265/libvpx/opus/lame) may still link dynamically from
Homebrew unless static archives are supplied (see "Fully static codecs"). To
ship a `.app`, bundle any remaining non-system dylibs and fix their install
names.

## Windows

Build self-contained under MSYS2 (provides the `sh`/`make`/`nasm` that FFmpeg's
configure requires):

```sh
# in an MSYS2 MINGW64 shell
pacman -S --needed base-devel make diffutils nasm yasm \
  mingw-w64-x86_64-toolchain mingw-w64-x86_64-rust \
  mingw-w64-x86_64-clang mingw-w64-x86_64-pkgconf \
  mingw-w64-x86_64-x264 mingw-w64-x86_64-x265 mingw-w64-x86_64-libvpx \
  mingw-w64-x86_64-opus mingw-w64-x86_64-lame
cargo build -p openconvert-app --release --locked --features static-ffmpeg
```

Windows static FFmpeg is the most involved path and may need toolchain tuning.
The alternative is prebuilt FFmpeg shared dev libraries (set `FFMPEG_DIR`) plus
shipping the DLLs beside `openconvert-app.exe`.

## Fully static codecs (Linux, optional)

`--features static-ffmpeg` statically links libav but the GPL codecs may remain
dynamic. To also fold them in, build their `.a` archives once and point
pkg-config at them:

```sh
bash scripts/build-static-codecs.sh

PKG_CONFIG_PATH="$PWD/target/static-codecs/lib/pkgconfig:$PWD/target/static-codecs/lib64/pkgconfig" \
PKG_CONFIG_ALL_STATIC=1 \
RUSTFLAGS="-C link-arg=-static-libstdc++ -C link-arg=-static-libgcc" \
cargo build -p openconvert-app --release --locked --features static-ffmpeg
```

After this the only dynamic dependencies are the Linux system boundary
(`libasound`, glibc). Verify with `readelf -d target/release/openconvert-app`.

## Desktop integration

Package the binary with platform integration files only, e.g.
`packaging/linux/openconvert.desktop` for Linux.

Open/save dialogs intentionally use system tools instead of a Rust dialog crate
to keep the dependency surface small:

- Linux: `zenity` first, then `kdialog`.
- Windows: PowerShell with `System.Windows.Forms`.

Release builds use the workspace release profile: size-optimized codegen, thin
LTO, `panic = "abort"`, and stripped symbols.
