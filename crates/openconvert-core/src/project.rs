use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::timeline::{Timeline, TimelineError};

/// Current OpenConvert project file format version.
pub const PROJECT_FORMAT_VERSION: u32 = 1;

/// Serializable OpenConvert project document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    /// Project file format version used for compatibility checks.
    pub format_version: u32,
    /// Human-readable project name.
    pub name: String,
    /// Editable media timeline stored by this project.
    pub timeline: Timeline,
}

/// Errors returned while loading, saving, or validating a project.
#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    /// Reading the project file failed.
    #[error("failed to read project file")]
    Read(#[source] io::Error),

    /// Writing the project file failed.
    #[error("failed to write project file")]
    Write(#[source] io::Error),

    /// Project JSON could not be parsed or serialized.
    #[error("project JSON is invalid")]
    Json(#[source] serde_json::Error),

    /// Project file was written for an unsupported format version.
    #[error("unsupported project format version {0}")]
    UnsupportedVersion(u32),

    /// Project timeline violates timeline invariants.
    #[error("project timeline is invalid")]
    Timeline(#[source] TimelineError),
}

impl Project {
    /// Creates an empty project with the current file format version.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            format_version: PROJECT_FORMAT_VERSION,
            name: name.into(),
            timeline: Timeline::new(),
        }
    }

    /// Parses and validates a project from JSON text.
    pub fn from_json_str(json: &str) -> Result<Self, ProjectError> {
        let project: Project = serde_json::from_str(json).map_err(ProjectError::Json)?;
        project.validate()?;

        Ok(project)
    }

    /// Serializes the project as pretty-printed JSON after validation.
    pub fn to_json_string(&self) -> Result<String, ProjectError> {
        self.validate()?;
        serde_json::to_string_pretty(self).map_err(ProjectError::Json)
    }

    /// Loads and validates a project from disk.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ProjectError> {
        let json = fs::read_to_string(path).map_err(ProjectError::Read)?;
        Self::from_json_str(&json)
    }

    /// Saves the project to disk after validation.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), ProjectError> {
        let json = self.to_json_string()?;
        fs::write(path, json).map_err(ProjectError::Write)
    }

    /// Validates project format compatibility and timeline invariants.
    pub fn validate(&self) -> Result<(), ProjectError> {
        if self.format_version != PROJECT_FORMAT_VERSION {
            return Err(ProjectError::UnsupportedVersion(self.format_version));
        }

        self.timeline.validate().map_err(ProjectError::Timeline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_round_trip_preserves_ids_paths_timing_and_version() {
        let mut project = Project::new("round trip");
        let track = project.timeline.add_track();
        let clip = project
            .timeline
            .add_clip(track, "media/phone clip.mp4".into(), 250, 1_000, 5_000)
            .unwrap();

        project.timeline.split_clip(track, clip, 2_250).unwrap();

        let json = project.to_json_string().unwrap();
        let loaded = Project::from_json_str(&json).unwrap();

        assert_eq!(loaded.format_version, PROJECT_FORMAT_VERSION);
        assert_eq!(loaded.name, "round trip");
        assert_eq!(loaded.timeline.tracks(), project.timeline.tracks());
    }

    #[test]
    fn load_rejects_unknown_future_version() {
        let json = r#"{
            "format_version": 99,
            "name": "future",
            "timeline": {
                "next_track_id": 1,
                "next_clip_id": 1,
                "tracks": []
            }
        }"#;

        assert!(matches!(
            Project::from_json_str(json),
            Err(ProjectError::UnsupportedVersion(99))
        ));
    }
}
