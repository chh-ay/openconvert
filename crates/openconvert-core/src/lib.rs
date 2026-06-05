//! Core project, timeline, and resource types for OpenConvert.

/// Project file loading, saving, and validation.
pub mod project;
/// Shared resource-management primitives.
pub mod resource;
/// Timeline editing model and errors.
pub mod timeline;

pub use project::{Project, ProjectError, PROJECT_FORMAT_VERSION};
pub use resource::{CancellationToken, FrameCache, ResourcePolicy};
pub use timeline::{Clip, ClipId, ClipKind, FitMode, Timeline, TimelineError, Track, TrackId};
