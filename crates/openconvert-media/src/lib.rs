//! Media probing, decoding, in-process rendering, and runtime media settings.

/// CPU compositing of decoded preview layers.
pub mod composite;
/// In-process video decoding for preview playback.
pub mod decode;
/// Export container, codec, and quality settings.
pub mod export;
/// Media performance and hardware-acceleration configuration.
pub mod performance;
/// Media metadata probing.
pub mod probe;
/// In-process video encoding and muxing for export.
pub mod render;

pub use composite::{composite_layers, CompositeLayer};
pub use decode::{decode_frame_at, DecodeError, DecodedFrame, ScrubDecoder, VideoDecoder};
pub use export::{CompressionPreset, Container, ExportOptions, VideoCodec};
pub use performance::{HwAccel, MediaPerformance};
pub use probe::{probe_media, MediaProbeError, MediaProbeResult};
pub use render::{render_timeline, RenderError};
