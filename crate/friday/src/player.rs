//! mpv wrapper: resolve a working executable, build a platform-correct
//! `Command`, and stage incoming MP3 data under a unique temporary path.

use std::{
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const LISTEN_ADDR: &str = "127.0.0.1:17322";
pub(super) const POLL_INTERVAL: Duration = Duration::from_millis(200);

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// `PATH` lookup for `mpv` (Windows: `mpv.exe` first).
pub(super) fn resolve_mpv() -> Option<String> {
    if let Ok(path) = std::env::var("FRIDAY_MPV")
        && !path.trim().is_empty()
    {
        return find_program(&path);
    }
    ["mpv", "mpv.exe"].into_iter().find_map(find_program)
}

/// Resolve an executable without launching it. Probing with `--version` can
/// hang on a broken executable and would make the Start/Stop UI unresponsive.
fn find_program(program: &str) -> Option<String> {
    let path = std::path::Path::new(program);
    if path.components().count() > 1 {
        return path.is_file().then(|| program.to_string());
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|directory| directory.join(program))
        .find(|candidate| candidate.is_file())
        .map(|candidate| candidate.to_string_lossy().into_owned())
}

#[cfg(not(target_os = "windows"))]
pub(super) fn player_command(program: &str) -> Command {
    use std::os::unix::process::CommandExt;

    let mut command = Command::new(program);
    // New session / process group: mpv stays headless and does not grab a TTY.
    command.process_group(0);
    command
}

#[cfg(target_os = "windows")]
pub(super) fn player_command(program: &str) -> Command {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = Command::new(program);
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

/// Configure silent, headless playback. Normalization is restricted to TTS:
/// applying it to raw microphone recordings raises their noise floor.
pub(super) fn configure_player_command(
    command: &mut Command,
    rate: f32,
    path: &std::path::Path,
    normalize_speech: bool,
) {
    command
        .arg("--no-config")
        .arg("--no-video")
        .arg("--really-quiet")
        .arg("--force-window=no")
        .arg("--osd-level=0")
        .arg("--no-terminal");
    if normalize_speech {
        command.arg("--af=lavfi=[loudnorm=I=-16:TP=-1.5:LRA=11]");
    }
    command
        .arg("--volume=100")
        .arg("--volume-max=100")
        .arg(format!("--speed={rate}"))
        .arg(path);
}

pub(super) fn temporary_mp3_path() -> PathBuf {
    temporary_audio_path("mp3")
}

fn temporary_audio_path(extension: &str) -> PathBuf {
    let id = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "friday-{}-{nanos}-{id}.{extension}",
        std::process::id()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_keeps_unity_gain() {
        let path = std::path::Path::new("voice.wav");
        let mut recording = Command::new("mpv");
        configure_player_command(&mut recording, 1.0, path, false);
        let recording = recording
            .get_args()
            .map(|argument| argument.to_string_lossy())
            .collect::<Vec<_>>();
        assert!(
            !recording
                .iter()
                .any(|argument| argument.starts_with("--af="))
        );
        assert!(recording.iter().any(|argument| argument == "--volume=100"));

        let mut speech = Command::new("mpv");
        configure_player_command(&mut speech, 1.0, path, true);
        assert!(
            speech
                .get_args()
                .any(|argument| argument == "--af=lavfi=[loudnorm=I=-16:TP=-1.5:LRA=11]")
        );
    }
}
