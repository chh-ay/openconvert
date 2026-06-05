//! In-process video/audio encoding and muxing via libav.
//!
//! This is the export-side counterpart to [`crate::decode`]: instead of building
//! an FFmpeg CLI command, it drives libavcodec/libavformat directly. Video is
//! composited per output frame from the same sources the preview uses (so export
//! is WYSIWYG); audio from every unmuted clip is decoded, resampled, mixed at its
//! timeline position, and encoded. It lets a statically linked binary export with
//! no external `ffmpeg` process.

use std::collections::HashMap;
use std::path::Path;

use ffmpeg::format::context::Output;
use ffmpeg::format::sample::Type as SampleType;
use ffmpeg::format::{Pixel, Sample};
use ffmpeg::software::resampling::Context as Resampler;
use ffmpeg::software::scaling::{context::Context as Scaler, flag::Flags};
use ffmpeg::util::frame::audio::Audio as AudioFrame;
use ffmpeg::util::frame::video::Video as VideoFrame;
use ffmpeg::{codec, encoder, format, media, ChannelLayout, Dictionary, Packet, Rational};
use ffmpeg_next as ffmpeg;
use openconvert_core::timeline::{Clip, ClipId, ClipKind, Timeline};

use crate::composite::{composite_layers, CompositeLayer};
use crate::decode::{ensure_init, DecodedFrame, VideoDecoder};
use crate::export::{output_video_size, Container, ExportOptions, VideoCodec};

/// Frames per second for in-process timeline rendering.
const RENDER_FPS: u32 = 30;
/// Sample rate for the mixed audio output.
const MIX_RATE: u32 = 48_000;

/// Errors returned while encoding or muxing an export.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// A libav call failed.
    #[error("ffmpeg encode error: {0}")]
    Ffmpeg(#[from] ffmpeg::Error),
    /// A source clip failed to decode.
    #[error("decode error: {0}")]
    Decode(#[from] crate::decode::DecodeError),
    /// No encoder is available for the requested codec.
    #[error("no encoder available for {0:?}")]
    NoEncoder(codec::Id),
    /// The timeline has nothing to render.
    #[error("nothing to render")]
    Empty,
}

/// Renders a timeline to `path` entirely in-process (no FFmpeg CLI).
///
/// Video frames composite the clips visible at each instant (bottom track first,
/// per-clip fit mode) via [`composite_layers`]; audio mixes every unmuted clip at
/// its timeline offset. The codecs follow `options.container`/`options.video_codec`.
pub fn render_timeline(
    timeline: &Timeline,
    path: &Path,
    options: ExportOptions,
) -> Result<(), RenderError> {
    ensure_init();

    let wants_video = video_codec_id(options.container, options.video_codec).is_some();
    let wants_audio = timeline_has_audio(timeline);
    if !wants_video && !wants_audio {
        return Err(RenderError::Empty);
    }

    let mut octx = format::output(&path)?;
    let global_header = octx.format().flags().contains(format::Flags::GLOBAL_HEADER);

    let canvas = output_video_size(timeline);
    let mut video = match video_codec_id(options.container, options.video_codec) {
        Some(codec_id) => Some(VideoTrack::new(
            &mut octx,
            codec_id,
            canvas,
            options,
            global_header,
        )?),
        None => None,
    };
    let mut audio = if wants_audio {
        Some(AudioTrack::new(
            &mut octx,
            audio_codec_id(options.container),
            options,
            global_header,
        )?)
    } else {
        None
    };

    octx.write_header()?;
    if let Some(video) = video.as_mut() {
        video.bind_stream_time_base(&octx);
    }
    if let Some(audio) = audio.as_mut() {
        audio.bind_stream_time_base(&octx);
    }

    let total_ms = timeline_duration_ms(timeline);

    // Pre-mix the whole timeline's audio: small (~384 KB/s) and lets audio be
    // emitted interleaved with video by timestamp.
    let mixed = if audio.is_some() {
        mix_timeline_audio(timeline, total_ms)?
    } else {
        StereoBuffer::default()
    };
    let mut audio_cursor = 0usize;

    if let Some(video) = video.as_mut() {
        let (width, height) = (video.width, video.height);
        let frame_count = total_ms
            .saturating_mul(u64::from(RENDER_FPS))
            .div_ceil(1_000);
        let mut readers: HashMap<ClipId, ClipReader> = HashMap::new();

        for frame in 0..frame_count {
            let at_ms = frame.saturating_mul(1_000) / u64::from(RENDER_FPS);

            // Emit audio that precedes this video frame so packets reach the
            // muxer in roughly timestamp order (bounded interleave buffering).
            if let Some(audio) = audio.as_mut() {
                let until = (u64::from(MIX_RATE) * at_ms / 1_000) as usize;
                audio.encode_until(&mut octx, &mixed, &mut audio_cursor, until)?;
            }

            let visible = timeline.video_layers_at(at_ms);
            for (track_id, clip_id) in &visible {
                let Some(clip) = timeline.clip(*track_id, *clip_id) else {
                    continue;
                };
                if !readers.contains_key(clip_id) {
                    readers.insert(*clip_id, ClipReader::open(clip, width, height)?);
                }
                let source_ms = clip
                    .source_start_ms
                    .saturating_add(at_ms.saturating_sub(clip.timeline_start_ms));
                readers
                    .get_mut(clip_id)
                    .expect("reader inserted above")
                    .advance_to(source_ms)?;
            }

            let mut layers = Vec::with_capacity(visible.len());
            for (track_id, clip_id) in &visible {
                let Some(clip) = timeline.clip(*track_id, *clip_id) else {
                    continue;
                };
                if let Some(decoded) = readers.get(clip_id).and_then(|r| r.current.as_ref()) {
                    layers.push(CompositeLayer {
                        rgba: &decoded.rgba,
                        width: decoded.width,
                        height: decoded.height,
                        fit: clip.fit_mode,
                    });
                }
            }

            let canvas = composite_layers(width, height, &layers);
            video.encode_frame(&mut octx, &canvas)?;

            readers.retain(|clip_id, _| visible.iter().any(|(_, id)| id == clip_id));
        }
    }

    // Flush any remaining audio, then both encoders.
    if let Some(audio) = audio.as_mut() {
        audio.encode_until(&mut octx, &mixed, &mut audio_cursor, mixed.frames())?;
        audio.finish(&mut octx)?;
    }
    if let Some(video) = video.as_mut() {
        video.finish(&mut octx)?;
    }

    octx.write_trailer()?;
    Ok(())
}

/// A video stream and its encoder within an output container.
struct VideoTrack {
    encoder: encoder::Video,
    scaler: Scaler,
    yuv: VideoFrame,
    width: u32,
    height: u32,
    stream_index: usize,
    encoder_time_base: Rational,
    stream_time_base: Rational,
    next_pts: i64,
}

impl VideoTrack {
    fn new(
        octx: &mut Output,
        codec_id: codec::Id,
        canvas: (u32, u32),
        options: ExportOptions,
        global_header: bool,
    ) -> Result<Self, RenderError> {
        let (width, height) = canvas;
        let codec = encoder::find(codec_id);
        let mut stream = octx.add_stream(codec)?;
        let time_base = Rational(1, RENDER_FPS as i32);

        let mut encoder =
            codec::context::Context::new_with_codec(codec.ok_or(RenderError::NoEncoder(codec_id))?)
                .encoder()
                .video()?;
        encoder.set_width(width);
        encoder.set_height(height);
        encoder.set_format(Pixel::YUV420P);
        encoder.set_time_base(time_base);
        encoder.set_frame_rate(Some(Rational(RENDER_FPS as i32, 1)));
        if global_header {
            encoder.set_flags(codec::Flags::GLOBAL_HEADER);
        }

        let encoder = encoder.open_with(video_options(options))?;
        stream.set_parameters(&encoder);
        let stream_index = stream.index();

        let scaler = Scaler::get(
            Pixel::RGBA,
            width,
            height,
            Pixel::YUV420P,
            width,
            height,
            Flags::BILINEAR,
        )?;

        Ok(Self {
            encoder,
            scaler,
            yuv: VideoFrame::empty(),
            width,
            height,
            stream_index,
            encoder_time_base: time_base,
            stream_time_base: time_base,
            next_pts: 0,
        })
    }

    fn bind_stream_time_base(&mut self, octx: &Output) {
        if let Some(stream) = octx.stream(self.stream_index) {
            self.stream_time_base = stream.time_base();
        }
    }

    fn encode_frame(&mut self, octx: &mut Output, rgba: &[u8]) -> Result<(), RenderError> {
        let mut source = VideoFrame::new(Pixel::RGBA, self.width, self.height);
        let stride = source.stride(0);
        let row_bytes = self.width as usize * 4;
        for (dst, src) in source
            .data_mut(0)
            .chunks_mut(stride)
            .zip(rgba.chunks_exact(row_bytes))
        {
            dst[..row_bytes].copy_from_slice(src);
        }

        self.scaler.run(&source, &mut self.yuv)?;
        self.yuv.set_pts(Some(self.next_pts));
        self.next_pts += 1;
        self.encoder.send_frame(&self.yuv)?;
        self.drain(octx)
    }

    fn finish(&mut self, octx: &mut Output) -> Result<(), RenderError> {
        self.encoder.send_eof()?;
        self.drain(octx)
    }

    fn drain(&mut self, octx: &mut Output) -> Result<(), RenderError> {
        let mut packet = Packet::empty();
        while self.encoder.receive_packet(&mut packet).is_ok() {
            packet.set_stream(self.stream_index);
            packet.rescale_ts(self.encoder_time_base, self.stream_time_base);
            packet.write_interleaved(octx)?;
        }
        Ok(())
    }
}

/// An audio stream and its encoder within an output container.
struct AudioTrack {
    encoder: encoder::Audio,
    resampler: Resampler,
    frame_size: usize,
    stream_index: usize,
    encoder_time_base: Rational,
    stream_time_base: Rational,
    next_pts: i64,
}

impl AudioTrack {
    fn new(
        octx: &mut Output,
        codec_id: codec::Id,
        options: ExportOptions,
        global_header: bool,
    ) -> Result<Self, RenderError> {
        let codec = encoder::find(codec_id)
            .ok_or(RenderError::NoEncoder(codec_id))?
            .audio()?;
        let sample_format = codec
            .formats()
            .and_then(|mut formats| formats.next())
            .unwrap_or(Sample::F32(SampleType::Planar));

        let mut stream = octx.add_stream(codec)?;
        let time_base = Rational(1, MIX_RATE as i32);

        let mut encoder = codec::context::Context::new_with_codec(*codec)
            .encoder()
            .audio()?;
        encoder.set_rate(MIX_RATE as i32);
        encoder.set_channel_layout(ChannelLayout::STEREO);
        encoder.set_format(sample_format);
        encoder.set_bit_rate(usize::from(options.audio_bitrate_kbps) * 1_000);
        encoder.set_time_base(time_base);
        if global_header {
            encoder.set_flags(codec::Flags::GLOBAL_HEADER);
        }

        let encoder = encoder.open_as(codec)?;
        stream.set_parameters(&encoder);
        let stream_index = stream.index();

        // 0 means the encoder accepts any frame size (e.g. PCM); pick a default.
        let frame_size = match encoder.frame_size() {
            0 => 1_024,
            size => size as usize,
        };

        let resampler = Resampler::get(
            Sample::F32(SampleType::Planar),
            ChannelLayout::STEREO,
            MIX_RATE,
            sample_format,
            ChannelLayout::STEREO,
            MIX_RATE,
        )?;

        Ok(Self {
            encoder,
            resampler,
            frame_size,
            stream_index,
            encoder_time_base: time_base,
            stream_time_base: time_base,
            next_pts: 0,
        })
    }

    fn bind_stream_time_base(&mut self, octx: &Output) {
        if let Some(stream) = octx.stream(self.stream_index) {
            self.stream_time_base = stream.time_base();
        }
    }

    /// Encodes full frames from `mixed[*cursor..until]`, advancing `cursor`.
    fn encode_until(
        &mut self,
        octx: &mut Output,
        mixed: &StereoBuffer,
        cursor: &mut usize,
        until: usize,
    ) -> Result<(), RenderError> {
        let until = until.min(mixed.frames());
        while *cursor < until {
            let take = self.frame_size.min(until - *cursor);
            if take < self.frame_size && until < mixed.frames() {
                // Wait for a full frame unless this is the tail of the stream.
                break;
            }
            self.encode_chunk(
                octx,
                &mixed.left[*cursor..*cursor + take],
                &mixed.right[*cursor..*cursor + take],
            )?;
            *cursor += take;
        }
        Ok(())
    }

    fn encode_chunk(
        &mut self,
        octx: &mut Output,
        left: &[f32],
        right: &[f32],
    ) -> Result<(), RenderError> {
        let mut planar = AudioFrame::new(
            Sample::F32(SampleType::Planar),
            left.len(),
            ChannelLayout::STEREO,
        );
        planar.set_rate(MIX_RATE);
        planar.plane_mut::<f32>(0).copy_from_slice(left);
        planar.plane_mut::<f32>(1).copy_from_slice(right);

        let mut frame = AudioFrame::empty();
        self.resampler.run(&planar, &mut frame)?;
        if frame.format() == Sample::None {
            // No output produced for this chunk (rare); skip it.
            return Ok(());
        }
        frame.set_pts(Some(self.next_pts));
        self.next_pts += frame.samples() as i64;

        self.encoder.send_frame(&frame)?;
        self.drain(octx)
    }

    fn finish(&mut self, octx: &mut Output) -> Result<(), RenderError> {
        self.encoder.send_eof()?;
        self.drain(octx)
    }

    fn drain(&mut self, octx: &mut Output) -> Result<(), RenderError> {
        let mut packet = Packet::empty();
        while self.encoder.receive_packet(&mut packet).is_ok() {
            packet.set_stream(self.stream_index);
            packet.rescale_ts(self.encoder_time_base, self.stream_time_base);
            packet.write_interleaved(octx)?;
        }
        Ok(())
    }
}

/// A stereo planar f32 audio buffer at [`MIX_RATE`].
#[derive(Default)]
struct StereoBuffer {
    left: Vec<f32>,
    right: Vec<f32>,
}

impl StereoBuffer {
    fn frames(&self) -> usize {
        self.left.len()
    }
}

/// Whether the timeline has any audio that should be exported.
fn timeline_has_audio(timeline: &Timeline) -> bool {
    timeline
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .any(|clip| clip.has_audio && !clip.muted && !matches!(clip.kind, ClipKind::Image))
}

/// Mixes every unmuted audio clip into one stereo planar buffer, positioned at
/// each clip's timeline offset.
fn mix_timeline_audio(timeline: &Timeline, total_ms: u64) -> Result<StereoBuffer, RenderError> {
    let total = (u64::from(MIX_RATE) * total_ms / 1_000) as usize;
    let mut buffer = StereoBuffer {
        left: vec![0.0; total],
        right: vec![0.0; total],
    };

    for track in timeline.tracks() {
        for clip in track.clips() {
            if !clip.has_audio || clip.muted || matches!(clip.kind, ClipKind::Image) {
                continue;
            }
            mix_clip_audio(clip, &mut buffer)?;
        }
    }
    Ok(buffer)
}

/// Decodes, resamples, and adds one clip's audio into the mix at its position.
fn mix_clip_audio(clip: &Clip, buffer: &mut StereoBuffer) -> Result<(), RenderError> {
    let mut input = ffmpeg::format::input(&Path::new(&clip.source_path))?;
    let Some(stream) = input.streams().best(media::Type::Audio) else {
        return Ok(());
    };
    let stream_index = stream.index();
    let mut decoder = codec::context::Context::from_parameters(stream.parameters())?
        .decoder()
        .audio()?;

    let mut resampler = None;

    // Seek to the clip's in-point; audio seek lands near, which is acceptable.
    let seek_ts = (clip.source_start_ms as i64).saturating_mul(1_000);
    let _ = input.seek(seek_ts, ..seek_ts);

    let max_frames = (u64::from(MIX_RATE) * clip.duration_ms / 1_000) as usize;
    let mut left = Vec::with_capacity(max_frames);
    let mut right = Vec::with_capacity(max_frames);

    let mut decoded = AudioFrame::empty();
    for (packet_stream, packet) in input.packets() {
        if packet_stream.index() != stream_index {
            continue;
        }
        decoder.send_packet(&packet)?;
        while decoder.receive_frame(&mut decoded).is_ok() {
            append_resampled(&mut resampler, &decoded, &mut left, &mut right)?;
        }
        if left.len() >= max_frames {
            break;
        }
    }
    decoder.send_eof()?;
    while decoder.receive_frame(&mut decoded).is_ok() {
        append_resampled(&mut resampler, &decoded, &mut left, &mut right)?;
    }

    let dest_start = (u64::from(MIX_RATE) * clip.timeline_start_ms / 1_000) as usize;
    let count = left.len().min(max_frames);
    for i in 0..count {
        let Some(slot) = dest_start.checked_add(i) else {
            break;
        };
        if slot >= buffer.left.len() {
            break;
        }
        buffer.left[slot] += left[i];
        buffer.right[slot] += right[i];
    }
    Ok(())
}

/// Resamples one decoded audio frame to stereo planar f32 and appends its samples.
fn append_resampled(
    resampler: &mut Option<Resampler>,
    decoded: &AudioFrame,
    left: &mut Vec<f32>,
    right: &mut Vec<f32>,
) -> Result<(), RenderError> {
    if resampler.is_none() {
        *resampler = Some(Resampler::get(
            decoded.format(),
            audio_frame_layout(decoded),
            decoded.rate(),
            Sample::F32(SampleType::Planar),
            ChannelLayout::STEREO,
            MIX_RATE,
        )?);
    }
    let Some(resampler) = resampler.as_mut() else {
        return Ok(());
    };
    let mut out = AudioFrame::empty();
    resampler.run(decoded, &mut out)?;
    if out.samples() == 0 {
        return Ok(());
    }
    left.extend_from_slice(out.plane::<f32>(0));
    right.extend_from_slice(out.plane::<f32>(1));
    Ok(())
}

fn audio_frame_layout(frame: &AudioFrame) -> ChannelLayout {
    if frame.channel_layout().channels() > 0 {
        frame.channel_layout()
    } else if frame.channels() > 0 {
        ChannelLayout::default(i32::from(frame.channels()))
    } else {
        ChannelLayout::STEREO
    }
}

/// Total timeline length: the latest clip end across every track.
fn timeline_duration_ms(timeline: &Timeline) -> u64 {
    timeline
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .map(|clip| clip.timeline_end_ms())
        .max()
        .unwrap_or(0)
}

/// The libav video encoder for a container/codec choice, or `None` for
/// audio-only output.
fn video_codec_id(container: Container, video_codec: VideoCodec) -> Option<codec::Id> {
    match container {
        Container::Mp4 | Container::Mkv | Container::Mov => Some(match video_codec {
            VideoCodec::H264 => codec::Id::H264,
            VideoCodec::H265 => codec::Id::HEVC,
        }),
        Container::WebM => Some(codec::Id::VP9),
        Container::Mp3 => None,
    }
}

/// The libav audio encoder for a container.
fn audio_codec_id(container: Container) -> codec::Id {
    match container {
        Container::WebM => codec::Id::OPUS,
        Container::Mp3 => codec::Id::MP3,
        _ => codec::Id::AAC,
    }
}

/// Encoder options (CRF/preset) for a container's video codec.
fn video_options(options: ExportOptions) -> Dictionary<'static> {
    let mut dict = Dictionary::new();
    match options.container {
        Container::WebM => {
            dict.set("crf", &options.video_quality.saturating_add(11).to_string());
            dict.set("b", "0");
        }
        Container::Mp3 => {}
        _ => {
            dict.set("preset", "veryfast");
            dict.set("crf", &options.video_quality.to_string());
        }
    }
    dict
}

/// A clip's video decoder advanced in lockstep with the render clock.
///
/// Output time only moves forward, so each clip decodes sequentially from its
/// in-point — frame-accurate, with no per-frame seeks.
struct ClipReader {
    decoder: VideoDecoder,
    current: Option<DecodedFrame>,
    pending: Option<DecodedFrame>,
    is_image: bool,
}

impl ClipReader {
    fn open(clip: &Clip, max_width: u32, max_height: u32) -> Result<Self, RenderError> {
        let mut decoder = VideoDecoder::open(Path::new(&clip.source_path), max_width, max_height)?;
        let is_image = matches!(clip.kind, ClipKind::Image);
        if !is_image {
            // Non-fatal: an unseekable source still decodes from the start.
            let _ = decoder.seek(clip.source_start_ms);
        }
        Ok(Self {
            decoder,
            current: None,
            pending: None,
            is_image,
        })
    }

    /// Advances `current` to the frame shown at source time `target_ms` — the
    /// latest decoded frame whose timestamp is at or before it.
    fn advance_to(&mut self, target_ms: u64) -> Result<(), RenderError> {
        if self.is_image {
            if self.current.is_none() {
                self.current = self.decoder.next_frame()?;
            }
            return Ok(());
        }
        loop {
            if self.pending.is_none() {
                self.pending = self.decoder.next_frame()?;
            }
            match &self.pending {
                Some(frame) if frame.pts_ms <= target_ms => self.current = self.pending.take(),
                _ => break,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::decode_frame_at;
    use crate::probe::probe_media;

    fn build_sample(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("sample.mp4");
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x240:rate=10:duration=1",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=44100:duration=1",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
                "-y",
            ])
            .arg(&path)
            .status()
            .expect("ffmpeg builds the test asset");
        assert!(status.success(), "ffmpeg failed to build the test asset");
        path
    }

    #[test]
    fn renders_a_timeline_clip_to_an_in_process_video() {
        let dir = tempfile::tempdir().unwrap();
        let sample = build_sample(dir.path());
        let output = dir.path().join("render.mp4");

        let mut timeline = Timeline::new();
        let track = timeline.add_track();
        timeline
            .add_clip(track, sample.to_string_lossy().into_owned(), 0, 0, 1_000)
            .unwrap();

        render_timeline(&timeline, &output, ExportOptions::default())
            .expect("the timeline renders in-process");

        let (width, height) = output_video_size(&timeline);
        let probe = probe_media(&output).expect("the rendered file is probeable");
        assert_eq!((probe.width, probe.height), (Some(width), Some(height)));
    }

    #[test]
    fn renders_a_timeline_with_an_audio_stream() {
        let dir = tempfile::tempdir().unwrap();
        let sample = build_sample(dir.path());
        let output = dir.path().join("render.mp4");

        let mut timeline = Timeline::new();
        let track = timeline.add_track();
        timeline
            .add_clip(track, sample.to_string_lossy().into_owned(), 0, 0, 1_000)
            .unwrap();

        render_timeline(&timeline, &output, ExportOptions::default())
            .expect("the timeline renders in-process");

        let probe = probe_media(&output).expect("the rendered file is probeable");
        assert!(probe.audio_codec.is_some());
    }

    #[test]
    fn renders_a_visible_non_black_frame_for_overlapping_clips() {
        let dir = tempfile::tempdir().unwrap();
        let sample = build_sample(dir.path());
        let output = dir.path().join("overlap.mp4");

        let mut timeline = Timeline::new();
        let lower = timeline.add_track();
        let upper = timeline.add_track();
        timeline
            .add_clip(lower, sample.to_string_lossy().into_owned(), 0, 0, 1_000)
            .unwrap();
        timeline
            .add_clip(upper, sample.to_string_lossy().into_owned(), 0, 0, 1_000)
            .unwrap();

        render_timeline(&timeline, &output, ExportOptions::default())
            .expect("the overlapping timeline renders in-process");

        let frame = decode_frame_at(&output, 500, 1_280, 720).expect("a middle frame decodes");
        let luminance: u64 = frame.rgba.iter().map(|&b| u64::from(b)).sum();
        assert!(luminance > 0, "exported frame must not be uniformly black");
    }

    #[test]
    fn renders_audio_only_mp3_without_a_video_stream() {
        let dir = tempfile::tempdir().unwrap();
        let sample = build_sample(dir.path());
        let output = dir.path().join("audio.mp3");

        let mut timeline = Timeline::new();
        let track = timeline.add_track();
        timeline
            .add_clip(track, sample.to_string_lossy().into_owned(), 0, 0, 1_000)
            .unwrap();

        let options = ExportOptions {
            container: Container::Mp3,
            ..ExportOptions::default()
        };
        render_timeline(&timeline, &output, options).expect("the audio-only timeline renders");

        let probe = probe_media(&output).expect("the rendered file is probeable");
        assert!(probe.audio_codec.is_some());
        assert_eq!(probe.width, None);
    }
}
