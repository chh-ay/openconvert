#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PREFIX="${OPENCONVERT_STATIC_CODECS_PREFIX:-$ROOT/target/static-codecs}"
SRC="${OPENCONVERT_STATIC_CODECS_SRC:-$ROOT/target/static-codecs-src}"
JOBS="${JOBS:-$(nproc)}"

mkdir -p "$PREFIX" "$SRC"

fetch_repo() {
  local name="$1"
  local url="$2"
  local rev="$3"
  local dir="$SRC/$name"
  if [[ ! -d "$dir/.git" ]]; then
    rm -rf "$dir"
    git init "$dir"
    git -C "$dir" remote add origin "$url"
  fi
  git -C "$dir" fetch --depth 1 origin "$rev"
  git -C "$dir" checkout --detach FETCH_HEAD
}

build_x264() {
  if [[ -f "$PREFIX/lib/libx264.a" ]]; then return; fi
  fetch_repo x264 https://code.videolan.org/videolan/x264.git 0480cb05fa188d37ae87e8f4fd8f1aea3711f7ee
  pushd "$SRC/x264" >/dev/null
  ./configure --prefix="$PREFIX" --enable-static --disable-cli --disable-opencl
  make -j"$JOBS"
  make install
  popd >/dev/null
}

build_x265() {
  if [[ -f "$PREFIX/lib/libx265.a" ]]; then return; fi
  fetch_repo x265 https://bitbucket.org/multicoreware/x265_git.git 9b057b1726e3b7d7bf4d109468c1871c65dc485e
  cmake -S "$SRC/x265/source" -B "$SRC/x265/build-static" -G Ninja \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_INSTALL_PREFIX="$PREFIX" \
    -DENABLE_SHARED=OFF \
    -DENABLE_CLI=OFF \
    -DENABLE_TESTS=OFF \
    -DENABLE_LIBNUMA=OFF \
    -DENABLE_HDR10_PLUS=OFF
  cmake --build "$SRC/x265/build-static" --target install
  mkdir -p "$PREFIX/lib/pkgconfig"
  printf '%s\n' \
    "prefix=$PREFIX" \
    'exec_prefix=${prefix}' \
    'libdir=${exec_prefix}/lib' \
    'includedir=${prefix}/include' \
    '' \
    'Name: x265' \
    'Description: H.265/HEVC video encoder' \
    'Version: 4.1' \
    'Libs: -L${libdir} -lx265 -lstdc++ -lm -lpthread' \
    'Cflags: -I${includedir}' \
    > "$PREFIX/lib/pkgconfig/x265.pc"
}


build_libvpx() {
  if [[ -f "$PREFIX/lib/libvpx.a" ]]; then return; fi
  fetch_repo libvpx https://chromium.googlesource.com/webm/libvpx 908e88c1aa6a12a86feb5d36a919c219c42f1e2c
  pushd "$SRC/libvpx" >/dev/null
  ./configure --prefix="$PREFIX" --disable-shared --enable-static --disable-examples --disable-tools --disable-docs --disable-unit-tests --as=nasm
  make -j"$JOBS"
  make install
  popd >/dev/null
}

build_opus() {
  if [[ -f "$PREFIX/lib/libopus.a" ]]; then return; fi
  fetch_repo opus https://github.com/xiph/opus.git f8f99516092f4311a9b0784f190ff982df8eb2e6
  cmake -S "$SRC/opus" -B "$SRC/opus/build-static" -G Ninja \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_INSTALL_PREFIX="$PREFIX" \
    -DBUILD_SHARED_LIBS=OFF \
    -DOPUS_BUILD_PROGRAMS=OFF \
    -DOPUS_BUILD_TESTING=OFF
  cmake --build "$SRC/opus/build-static" --target install
}

build_lame() {
  if [[ -f "$PREFIX/lib/libmp3lame.a" ]]; then return; fi
  fetch_repo lame https://github.com/rbrito/lame.git af984672a95bb0eedc9f3193604e269c97cca162
  pushd "$SRC/lame" >/dev/null
  if [[ ! -x ./configure ]]; then
    ./configure --version >/dev/null 2>&1 || autoreconf -fi
  fi
  ./configure --prefix="$PREFIX" --enable-static --disable-shared --disable-frontend --disable-gtktest
  make -j"$JOBS"
  make install
  popd >/dev/null

  mkdir -p "$PREFIX/lib/pkgconfig"
  if [[ ! -f "$PREFIX/lib/pkgconfig/libmp3lame.pc" ]]; then
    printf '%s\n' \
      "prefix=$PREFIX" \
      'exec_prefix=${prefix}' \
      'libdir=${exec_prefix}/lib' \
      'includedir=${prefix}/include' \
      '' \
      'Name: libmp3lame' \
      'Description: LAME MP3 encoder' \
      'Version: 3.100' \
      'Libs: -L${libdir} -lmp3lame' \
      'Cflags: -I${includedir}' \
      > "$PREFIX/lib/pkgconfig/libmp3lame.pc"
  fi
}

build_x264
build_x265
build_libvpx
build_opus
build_lame

printf '\nStatic codec prefix: %s\n' "$PREFIX"
printf 'Use:\n'
printf '  export PKG_CONFIG_PATH=%s/lib/pkgconfig:%s/lib64/pkgconfig:$PKG_CONFIG_PATH\n' "$PREFIX" "$PREFIX"
printf '  export PKG_CONFIG_ALL_STATIC=1\n'
