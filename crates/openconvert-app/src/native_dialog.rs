use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogMode {
    OpenFile,
    SaveFile,
}

pub fn open_project() -> Option<PathBuf> {
    pick_path(
        DialogMode::OpenFile,
        "Open OpenConvert project",
        Some("*.ocproj *.json"),
    )
}

pub fn save_project() -> Option<PathBuf> {
    pick_path(
        DialogMode::SaveFile,
        "Save OpenConvert project",
        Some("*.ocproj"),
    )
}

pub fn open_media() -> Option<PathBuf> {
    pick_path(
        DialogMode::OpenFile,
        "Import local video or audio",
        Some("*.mp4 *.mov *.mkv *.webm *.m4v *.avi *.mp3 *.wav *.flac *.aac *.m4a *.ogg *.opus"),
    )
}

pub fn save_export() -> Option<PathBuf> {
    pick_path(
        DialogMode::SaveFile,
        "Export / convert media",
        Some("*.mp4 *.mkv *.webm *.mov *.mp3"),
    )
}

fn pick_path(mode: DialogMode, title: &str, pattern: Option<&str>) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        linux_pick_path(mode, title, pattern)
    }

    #[cfg(target_os = "macos")]
    {
        let _ = pattern;
        macos_pick_path(mode, title)
    }

    #[cfg(target_os = "windows")]
    {
        let _ = pattern;
        windows_pick_path(mode, title)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = (mode, title, pattern);
        None
    }
}

#[cfg(target_os = "linux")]
fn linux_pick_path(mode: DialogMode, title: &str, pattern: Option<&str>) -> Option<PathBuf> {
    let zenity = linux_zenity(mode, title, pattern);

    if zenity.is_some() {
        return zenity;
    }

    linux_kdialog(mode, title)
}

#[cfg(target_os = "linux")]
fn linux_zenity(mode: DialogMode, title: &str, pattern: Option<&str>) -> Option<PathBuf> {
    let mut command = Command::new("zenity");
    command.arg("--file-selection").arg("--title").arg(title);

    if mode == DialogMode::SaveFile {
        command.arg("--save").arg("--confirm-overwrite");
    }

    if let Some(pattern) = pattern {
        command.arg("--file-filter").arg(pattern);
    }

    output_path(command)
}

#[cfg(target_os = "linux")]
fn linux_kdialog(mode: DialogMode, title: &str) -> Option<PathBuf> {
    let mut command = Command::new("kdialog");
    command.arg("--title").arg(title);

    match mode {
        DialogMode::OpenFile => {
            command.arg("--getopenfilename");
        }
        DialogMode::SaveFile => {
            command.arg("--getsavefilename");
        }
    }

    output_path(command)
}

#[cfg(target_os = "windows")]
fn windows_pick_path(mode: DialogMode, title: &str) -> Option<PathBuf> {
    let script = match mode {
        DialogMode::OpenFile => windows_open_file_script(title),
        DialogMode::SaveFile => windows_save_file_script(title),
    };

    let mut command = Command::new("powershell.exe");
    command
        .arg("-NoProfile")
        .arg("-Sta")
        .arg("-Command")
        .arg(script);

    output_path(command)
}

#[cfg(target_os = "windows")]
fn windows_open_file_script(title: &str) -> String {
    format!(
        "Add-Type -AssemblyName System.Windows.Forms; \
         $d = New-Object System.Windows.Forms.OpenFileDialog; \
         $d.Title = '{}'; \
         if ($d.ShowDialog() -eq 'OK') {{ $d.FileName }}",
        powershell_escape(title)
    )
}

#[cfg(target_os = "windows")]
fn windows_save_file_script(title: &str) -> String {
    format!(
        "Add-Type -AssemblyName System.Windows.Forms; \
         $d = New-Object System.Windows.Forms.SaveFileDialog; \
         $d.Title = '{}'; \
         if ($d.ShowDialog() -eq 'OK') {{ $d.FileName }}",
        powershell_escape(title)
    )
}

#[cfg(target_os = "windows")]
fn powershell_escape(value: &str) -> String {
    value.replace('\'', "''")
}

#[cfg(target_os = "macos")]
fn macos_pick_path(mode: DialogMode, title: &str) -> Option<PathBuf> {
    // AppleScript's `choose file` / `choose file name` are the standard macOS
    // native pickers; `POSIX path of` prints the selection to stdout, and a
    // cancel exits non-zero so `output_path` maps it to `None`.
    let prompt = applescript_escape(title);
    let script = match mode {
        DialogMode::OpenFile => {
            format!("POSIX path of (choose file with prompt \"{prompt}\")")
        }
        DialogMode::SaveFile => {
            format!("POSIX path of (choose file name with prompt \"{prompt}\")")
        }
    };

    let mut command = Command::new("osascript");
    command.arg("-e").arg(script);

    output_path(command)
}

#[cfg(target_os = "macos")]
fn applescript_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn output_path(mut command: Command) -> Option<PathBuf> {
    let output = command.output().ok()?;

    if !output.status.success() {
        return None;
    }

    let path = String::from_utf8(output.stdout).ok()?;
    let path = path.trim();

    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
}
