use std::process::Command;
use std::sync::LazyLock;

/// Runtime media-processing settings derived from host capabilities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaPerformance {
    /// Number of application worker threads to use for media tasks.
    pub worker_threads: usize,
    /// Number of threads to pass to FFmpeg.
    pub ffmpeg_threads: usize,
    /// Hardware acceleration mode to request from FFmpeg.
    pub hwaccel: HwAccel,
}

/// FFmpeg hardware acceleration mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HwAccel {
    /// Let FFmpeg choose an available hardware accelerator.
    Auto,
    /// Request NVIDIA CUDA acceleration.
    Cuda,
    /// Request Linux VAAPI acceleration.
    Vaapi,
    /// Do not pass a hardware acceleration argument.
    None,
}

impl HwAccel {
    /// Returns the FFmpeg `-hwaccel` argument value, if one should be passed.
    pub fn ffmpeg_name(self) -> Option<&'static str> {
        match self {
            HwAccel::Auto => Some("auto"),
            HwAccel::Cuda => Some("cuda"),
            HwAccel::Vaapi => Some("vaapi"),
            HwAccel::None => None,
        }
    }
}

impl Default for MediaPerformance {
    fn default() -> Self {
        let cpu_threads = std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(1);

        Self {
            worker_threads: cpu_threads.saturating_sub(1).max(1),
            ffmpeg_threads: cpu_threads,
            hwaccel: *HWACCEL,
        }
    }
}

impl MediaPerformance {
    /// Appends FFmpeg input arguments that should precede each `-i` input.
    pub fn push_ffmpeg_input_args(&self, args: &mut Vec<String>) {
        if let Some(hwaccel) = self.hwaccel.ffmpeg_name() {
            args.push("-hwaccel".to_owned());
            args.push(hwaccel.to_owned());
        }
    }

    /// Appends FFmpeg thread-count arguments.
    pub fn push_ffmpeg_thread_args(&self, args: &mut Vec<String>) {
        args.push("-threads".to_owned());
        args.push(self.ffmpeg_threads.to_string());
    }
}

static HWACCEL: LazyLock<HwAccel> = LazyLock::new(detect_hwaccel);

fn detect_hwaccel() -> HwAccel {
    if command_exists("nvidia-smi") {
        return HwAccel::Cuda;
    }

    if std::path::Path::new("/dev/dri/renderD128").exists() {
        return HwAccel::Vaapi;
    }

    HwAccel::Auto
}

fn command_exists(name: &str) -> bool {
    Command::new(name)
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}
