use std::path::PathBuf;

use openconvert_core::Project;
use openconvert_media::{DecodedFrame, ExportOptions};

use crate::player::PlaybackTarget;
use crate::preview::PreviewKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Edit,
    Convert,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PreviewKind {
    Timeline,
    Convert,
}

impl From<PlaybackTarget> for PreviewKind {
    fn from(target: PlaybackTarget) -> Self {
        match target {
            PlaybackTarget::Timeline => PreviewKind::Timeline,
            PlaybackTarget::Convert => PreviewKind::Convert,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PreviewRequest {
    pub kind: PreviewKind,
    pub media_path: PathBuf,
    /// Bucketed source position to extract, in milliseconds.
    pub media_ms: u64,
    /// Cache key the extracted frame is stored under.
    pub key: PreviewKey,
}

#[derive(Debug)]
pub enum AppMessage {
    Status(String),
    Error(String),
    ProjectOpened {
        path: PathBuf,
        project: Project,
    },
    MediaImported {
        path: PathBuf,
        duration_ms: u64,
        has_audio: bool,
        width: u32,
        height: u32,
        is_image: bool,
        preview_frame: Option<DecodedFrame>,
    },
    PreviewReady {
        key: PreviewKey,
        frame: DecodedFrame,
    },
    PreviewFailed {
        error: String,
    },
    ThumbReady {
        key: PreviewKey,
        frame: DecodedFrame,
    },
    ThumbFailed {
        key: PreviewKey,
    },
    ExportFinished {
        path: PathBuf,
        options: ExportOptions,
    },
    ConvertInputSelected {
        path: PathBuf,
        duration_ms: u64,
        has_audio: bool,
        width: u32,
        height: u32,
        is_image: bool,
        preview_frame: Option<DecodedFrame>,
    },
    ConvertFinished {
        input: PathBuf,
        output: PathBuf,
        options: ExportOptions,
    },
}
