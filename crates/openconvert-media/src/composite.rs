//! CPU compositing of decoded preview layers into a single frame.
//!
//! Playback decodes each visible track-clip independently; this module blends
//! those layers into one RGBA frame the editor uploads as the preview texture.
//! It mirrors the export compositor (see [`crate::export`]) so what plays back
//! matches what renders: track order is bottom-to-top, fit modes letterbox /
//! crop / stretch identically, and uncovered area reveals the layers beneath.

use openconvert_core::FitMode;

/// One source layer to composite: tightly packed RGBA8 plus its pixel size and
/// how it maps into the canvas.
pub struct CompositeLayer<'a> {
    /// Tightly packed RGBA8 pixels (`width * height * 4` bytes).
    pub rgba: &'a [u8],
    /// Layer width in pixels.
    pub width: u32,
    /// Layer height in pixels.
    pub height: u32,
    /// How the layer maps into the canvas.
    pub fit: FitMode,
}

/// Composites `layers` bottom-to-top onto an opaque black canvas of
/// `canvas_width`×`canvas_height`, returning tightly packed RGBA8.
///
/// Each layer is placed per its [`FitMode`] — `Contain` letterboxes (uncovered
/// canvas keeps whatever is beneath), `Cover` fills and crops, `Stretch` fills
/// exactly — then alpha-blended over the canvas. The first layer is the bottom.
pub fn composite_layers(
    canvas_width: u32,
    canvas_height: u32,
    layers: &[CompositeLayer],
) -> Vec<u8> {
    let cw = canvas_width.max(1) as usize;
    let ch = canvas_height.max(1) as usize;

    let mut canvas = vec![0u8; cw * ch * 4];
    for pixel in canvas.chunks_exact_mut(4) {
        pixel[3] = 255;
    }

    for layer in layers {
        blend_layer(&mut canvas, cw, ch, layer);
    }
    canvas
}

/// Maps source coordinates to canvas coordinates: scaled source size `(dest_w,
/// dest_h)` placed at `(offset_x, offset_y)`. Offsets are negative for `Cover`,
/// where the scaled source overflows the canvas and is cropped.
fn placement(fit: FitMode, lw: f32, lh: f32, cw: f32, ch: f32) -> (f32, f32, f32, f32) {
    let (scale_x, scale_y) = match fit {
        FitMode::Stretch => (cw / lw, ch / lh),
        FitMode::Contain => {
            let scale = (cw / lw).min(ch / lh);
            (scale, scale)
        }
        FitMode::Cover => {
            let scale = (cw / lw).max(ch / lh);
            (scale, scale)
        }
    };
    let dest_w = lw * scale_x;
    let dest_h = lh * scale_y;
    ((cw - dest_w) * 0.5, (ch - dest_h) * 0.5, scale_x, scale_y)
}

fn blend_layer(canvas: &mut [u8], cw: usize, ch: usize, layer: &CompositeLayer) {
    let lw = layer.width as usize;
    let lh = layer.height as usize;
    if lw == 0 || lh == 0 || layer.rgba.len() < lw * lh * 4 {
        return;
    }

    let (offset_x, offset_y, scale_x, scale_y) =
        placement(layer.fit, lw as f32, lh as f32, cw as f32, ch as f32);

    // Only the canvas rows/columns the scaled layer actually covers.
    let x0 = offset_x.max(0.0).floor() as usize;
    let y0 = offset_y.max(0.0).floor() as usize;
    let x1 = ((offset_x + lw as f32 * scale_x).ceil() as usize).min(cw);
    let y1 = ((offset_y + lh as f32 * scale_y).ceil() as usize).min(ch);

    for cy in y0..y1 {
        let sy = ((cy as f32 + 0.5 - offset_y) / scale_y) as i32;
        if sy < 0 || sy as usize >= lh {
            continue;
        }
        let row = sy as usize * lw;
        for cx in x0..x1 {
            let sx = ((cx as f32 + 0.5 - offset_x) / scale_x) as i32;
            if sx < 0 || sx as usize >= lw {
                continue;
            }
            let source = &layer.rgba[(row + sx as usize) * 4..(row + sx as usize) * 4 + 4];
            let alpha = u32::from(source[3]);
            if alpha == 0 {
                continue;
            }
            let dest = (cy * cw + cx) * 4;
            if alpha == 255 {
                canvas[dest..dest + 4].copy_from_slice(source);
            } else {
                let inverse = 255 - alpha;
                for channel in 0..3 {
                    canvas[dest + channel] = ((u32::from(source[channel]) * alpha
                        + u32::from(canvas[dest + channel]) * inverse)
                        / 255) as u8;
                }
                canvas[dest + 3] = 255;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
        color
            .iter()
            .copied()
            .cycle()
            .take(width as usize * height as usize * 4)
            .collect()
    }

    fn pixel(canvas: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let index = (y as usize * width as usize + x as usize) * 4;
        [
            canvas[index],
            canvas[index + 1],
            canvas[index + 2],
            canvas[index + 3],
        ]
    }

    #[test]
    fn stretch_fills_the_entire_canvas_with_the_layer() {
        let red = solid(2, 2, [255, 0, 0, 255]);
        let layer = CompositeLayer {
            rgba: &red,
            width: 2,
            height: 2,
            fit: FitMode::Stretch,
        };

        let canvas = composite_layers(4, 4, &[layer]);

        assert_eq!(pixel(&canvas, 4, 0, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn contain_leaves_the_background_visible_in_the_letterbox() {
        // A 4x2 (wide) layer contained in a 4x4 canvas leaves top/bottom bars.
        let green = solid(4, 2, [0, 255, 0, 255]);
        let layer = CompositeLayer {
            rgba: &green,
            width: 4,
            height: 2,
            fit: FitMode::Contain,
        };

        let canvas = composite_layers(4, 4, &[layer]);

        // Top-left corner is in the letterbox and keeps the black background.
        assert_eq!(pixel(&canvas, 4, 0, 0), [0, 0, 0, 255]);
    }

    #[test]
    fn cover_fills_the_canvas_leaving_no_background() {
        // A 4x2 layer covering a 4x4 canvas scales up and crops; no black bars.
        let green = solid(4, 2, [0, 255, 0, 255]);
        let layer = CompositeLayer {
            rgba: &green,
            width: 4,
            height: 2,
            fit: FitMode::Cover,
        };

        let canvas = composite_layers(4, 4, &[layer]);

        assert_eq!(pixel(&canvas, 4, 0, 0), [0, 255, 0, 255]);
    }

    #[test]
    fn an_opaque_upper_layer_hides_the_layer_beneath_it() {
        let bottom = solid(2, 2, [0, 0, 255, 255]);
        let top = solid(2, 2, [255, 0, 0, 255]);
        let layers = [
            CompositeLayer {
                rgba: &bottom,
                width: 2,
                height: 2,
                fit: FitMode::Stretch,
            },
            CompositeLayer {
                rgba: &top,
                width: 2,
                height: 2,
                fit: FitMode::Stretch,
            },
        ];

        let canvas = composite_layers(2, 2, &layers);

        assert_eq!(pixel(&canvas, 2, 0, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn a_semi_transparent_upper_layer_blends_over_the_lower_layer() {
        let bottom = solid(1, 1, [0, 0, 0, 255]);
        let top = solid(1, 1, [255, 255, 255, 128]);
        let layers = [
            CompositeLayer {
                rgba: &bottom,
                width: 1,
                height: 1,
                fit: FitMode::Stretch,
            },
            CompositeLayer {
                rgba: &top,
                width: 1,
                height: 1,
                fit: FitMode::Stretch,
            },
        ];

        let canvas = composite_layers(1, 1, &layers);

        // 255*128/255 + 0 = 128 on each channel.
        assert_eq!(pixel(&canvas, 1, 0, 0), [128, 128, 128, 255]);
    }
}
