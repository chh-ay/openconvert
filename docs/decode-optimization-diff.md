# Decode Engine Optimization Diff

This note explains the optimization made in `crates/openconvert-media/src/decode.rs`.

## What Changed

The decoder now does three things more efficiently:

1. Enables FFmpeg frame threading for video decode.
2. Reuses internal FFmpeg video frame wrappers across `next_frame()` calls.
3. Skips row-by-row RGBA packing when FFmpeg already gives a tightly packed output plane.

The public API did not change. `VideoDecoder::open`, `VideoDecoder::seek`, `VideoDecoder::next_frame`, and `decode_frame_at` keep the same caller contract.

## Why It Helps

Before this change, every decoded frame created:

- a fresh decoded `Video` wrapper inside `next_frame`
- a fresh RGBA `Video` wrapper inside `scale`
- a row-by-row copy even when the RGBA output had no stride padding

For playback preview, this happens repeatedly on a background decoder thread, so reducing per-frame allocation and copy overhead matters. The final `Vec<u8>` is still required because decoded frames are sent through a channel and consumed later by the UI.

FFmpeg frame threading is capped at 4 threads so preview decode can use multiple cores without aggressively competing with the UI and other media work.

## Unified Diff

```diff
--- a/crates/openconvert-media/src/decode.rs
+++ b/crates/openconvert-media/src/decode.rs
@@
-use ffmpeg::format::{context::Input, input, Pixel};
+use ffmpeg::codec::threading;
+use ffmpeg::format::{context::Input, input, Pixel};
@@
 pub struct VideoDecoder {
     input: Input,
     decoder: ffmpeg::decoder::Video,
     scaler: Scaler,
+    decoded_frame: Video,
+    rgba_frame: Video,
     stream_index: usize,
     seconds_per_tick: f64,
     eof: bool,
 }
@@
-        let decoder_ctx = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
+        let mut decoder_ctx =
+            ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
+        decoder_ctx.set_threading(threading::Config {
+            kind: threading::Type::Frame,
+            count: decoder_threads(),
+        });
         let decoder = decoder_ctx.decoder().video()?;
@@
             input,
             decoder,
             scaler,
+            decoded_frame: Video::empty(),
+            rgba_frame: Video::empty(),
             stream_index,
             seconds_per_tick,
             eof: false,
         })
@@
     pub fn next_frame(&mut self) -> Result<Option<DecodedFrame>, DecodeError> {
         loop {
-            let mut frame = Video::empty();
-            if self.decoder.receive_frame(&mut frame).is_ok() {
-                return Ok(Some(self.scale(&frame)?));
+            if self.decoder.receive_frame(&mut self.decoded_frame).is_ok() {
+                let ticks = self
+                    .decoded_frame
+                    .pts()
+                    .or_else(|| self.decoded_frame.timestamp())
+                    .unwrap_or(0);
+                return self.scale(ticks).map(Some);
             }
             if self.eof {
                 return Ok(None);
             }
@@
-    fn scale(&mut self, frame: &Video) -> Result<DecodedFrame, DecodeError> {
-        let mut rgba = Video::empty();
-        self.scaler.run(frame, &mut rgba)?;
+    fn scale(&mut self, ticks: i64) -> Result<DecodedFrame, DecodeError> {
+        self.scaler.run(&self.decoded_frame, &mut self.rgba_frame)?;
 
-        let width = rgba.width();
-        let height = rgba.height();
+        let width = self.rgba_frame.width();
+        let height = self.rgba_frame.height();
         let row_bytes = width as usize * 4;
-        let stride = rgba.stride(0);
-        let source = rgba.data(0);
+        let stride = self.rgba_frame.stride(0);
+        let source = self.rgba_frame.data(0);
 
         // sws may pad rows to an alignment boundary; copy row-by-row so the
         // output is tightly packed for direct GPU upload.
-        let mut packed = vec![0u8; row_bytes * height as usize];
-        for y in 0..height as usize {
-            let src_start = y * stride;
-            packed[y * row_bytes..(y + 1) * row_bytes]
-                .copy_from_slice(&source[src_start..src_start + row_bytes]);
-        }
-
-        let ticks = frame.pts().or_else(|| frame.timestamp()).unwrap_or(0);
+        let packed_len = row_bytes * height as usize;
+        let rgba = if stride == row_bytes {
+            source[..packed_len].to_vec()
+        } else {
+            let mut packed = vec![0u8; packed_len];
+            for y in 0..height as usize {
+                let src_start = y * stride;
+                packed[y * row_bytes..(y + 1) * row_bytes]
+                    .copy_from_slice(&source[src_start..src_start + row_bytes]);
+            }
+            packed
+        };
         let pts_ms = (ticks as f64 * self.seconds_per_tick * 1_000.0).max(0.0) as u64;
 
         Ok(DecodedFrame {
             pts_ms,
             width,
             height,
-            rgba: packed,
+            rgba,
         })
     }
 }
+
+fn decoder_threads() -> usize {
+    std::thread::available_parallelism()
+        .map(|count| count.get().clamp(1, 4))
+        .unwrap_or(1)
+}
```

## Verification

These checks passed after the optimization:

```text
cargo fmt --check
cargo test -p openconvert-media
cargo clippy -p openconvert-media --all-targets -- -D warnings
```
