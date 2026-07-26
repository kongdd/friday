//! GUI-facing audio recorder: capture lifecycle, elapsed time, and local replay.

use std::{
    path::{Path, PathBuf},
    process::Child,
    time::{Duration, Instant},
};

use chrono::Local;

use crate::{
    capture::{self, Capture},
    player::{configure_player_command, player_command, resolve_mpv},
};

pub struct FridayRecorder {
    state: RecordingState,
    capture: Option<Capture>,
    path: Option<PathBuf>,
    player: Option<Child>,
    error: Option<String>,
    elapsed: Duration,
    active_since: Option<Instant>,
}

impl FridayRecorder {
    pub fn new() -> Self {
        Self {
            state: RecordingState::Idle,
            capture: None,
            path: None,
            player: None,
            error: None,
            elapsed: Duration::ZERO,
            active_since: None,
        }
    }

    pub fn state(&self) -> RecordingState {
        self.state
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn elapsed(&self) -> Duration {
        self.elapsed
            + self
                .active_since
                .map(|started| started.elapsed())
                .unwrap_or_default()
    }

    pub fn is_active(&self) -> bool {
        matches!(
            self.state,
            RecordingState::Recording | RecordingState::Paused
        )
    }

    pub fn is_playing(&self) -> bool {
        self.player.is_some()
    }

    pub fn level(&self) -> f32 {
        self.capture
            .as_ref()
            .map(Capture::level)
            .unwrap_or_default()
    }

    pub fn start(&mut self) -> Result<(), String> {
        ensure_transition(self.state, RecordAction::Start)?;
        // Prevent replay from feeding back into the new microphone capture.
        self.stop_playback();
        let path = match recording_wav_path() {
            Ok(path) => path,
            Err(error) => {
                self.fail(error.clone());
                return Err(error);
            }
        };
        let capture = match capture::start(&path) {
            Ok(capture) => capture,
            Err(error) => {
                let _ = std::fs::remove_file(&path);
                self.fail(error.clone());
                return Err(error);
            }
        };

        self.path = Some(path);
        self.capture = Some(capture);
        self.state = RecordingState::Recording;
        self.error = None;
        self.elapsed = Duration::ZERO;
        self.active_since = Some(Instant::now());
        Ok(())
    }

    pub fn pause(&mut self) -> Result<(), String> {
        ensure_transition(self.state, RecordAction::Pause)?;
        self.capture
            .as_mut()
            .ok_or_else(|| "recording session is missing".to_string())?
            .pause()
            .inspect_err(|error| self.error = Some(error.clone()))?;
        self.freeze_elapsed();
        self.state = RecordingState::Paused;
        Ok(())
    }

    pub fn resume(&mut self) -> Result<(), String> {
        ensure_transition(self.state, RecordAction::Resume)?;
        self.capture
            .as_mut()
            .ok_or_else(|| "recording session is missing".to_string())?
            .resume()
            .inspect_err(|error| self.error = Some(error.clone()))?;
        self.active_since = Some(Instant::now());
        self.state = RecordingState::Recording;
        Ok(())
    }

    pub fn finish(&mut self) -> Result<(), String> {
        ensure_transition(self.state, RecordAction::Finish)?;
        self.freeze_elapsed();
        let result = self
            .capture
            .take()
            .ok_or_else(|| "recording session is missing".to_string())?
            .finish();
        match result {
            Ok(()) => {
                self.state = RecordingState::Finished;
                self.error = None;
                Ok(())
            }
            Err(error) => {
                self.fail(error.clone());
                Err(error)
            }
        }
    }

    pub fn play(&mut self) -> Result<(), String> {
        ensure_transition(self.state, RecordAction::Play)?;
        let path = self
            .path
            .clone()
            .ok_or_else(|| "no recording to play".to_string())?;
        let player = resolve_mpv().ok_or_else(|| {
            "mpv not found; install it or set FRIDAY_MPV to its executable".to_string()
        })?;
        self.stop_playback();
        let mut command = player_command(&player);
        configure_player_command(&mut command, 1.0, &path, false);
        self.player = Some(
            command
                .spawn()
                .map_err(|error| format!("cannot start mpv: {error}"))?,
        );
        Ok(())
    }

    pub fn stop_playback(&mut self) {
        if let Some(mut player) = self.player.take() {
            let _ = player.kill();
            let _ = player.wait();
        }
    }

    /// Called once per egui frame while capture or replay is active.
    pub fn poll(&mut self) {
        let playback_finished = self
            .player
            .as_mut()
            .is_some_and(|player| !matches!(player.try_wait(), Ok(None)));
        if playback_finished {
            self.player.take();
        }

        let capture_error = self.capture.as_mut().and_then(Capture::poll_error);
        if let Some(error) = capture_error {
            self.freeze_elapsed();
            self.capture.take();
            self.fail(error);
        }
    }

    fn freeze_elapsed(&mut self) {
        if let Some(started) = self.active_since.take() {
            self.elapsed += started.elapsed();
        }
    }

    fn fail(&mut self, error: String) {
        self.state = RecordingState::Failed;
        self.error = Some(error);
        self.active_since = None;
    }
}

impl Default for FridayRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for FridayRecorder {
    fn drop(&mut self) {
        self.stop_playback();
        if let Some(capture) = self.capture.take() {
            let _ = capture.finish();
        }
    }
}

fn recording_wav_path() -> Result<PathBuf, String> {
    let executable =
        std::env::current_exe().map_err(|error| format!("cannot locate executable: {error}"))?;
    let media = executable
        .parent()
        .ok_or_else(|| "executable directory is missing".to_string())?
        .join("media");
    std::fs::create_dir_all(&media)
        .map_err(|error| format!("cannot create {}: {error}", media.display()))?;
    next_recording_path(&media, &Local::now().format("%Y%m%d").to_string())
}

fn next_recording_path(media: &Path, date: &str) -> Result<PathBuf, String> {
    let prefix = format!("{date}_");
    let number = std::fs::read_dir(media)
        .map_err(|error| format!("cannot read {}: {error}", media.display()))?
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|name| {
            name.strip_prefix(&prefix)?
                .strip_suffix(".wav")?
                .parse::<u32>()
                .ok()
        })
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| format!("recording number overflow in {}", media.display()))?;
    Ok(media.join(format!("{date}_{number:03}.wav")))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RecordingState {
    #[default]
    Idle,
    Recording,
    Paused,
    Finished,
    Failed,
}

#[derive(Clone, Copy)]
enum RecordAction {
    Start,
    Pause,
    Resume,
    Finish,
    Play,
}

fn ensure_transition(state: RecordingState, action: RecordAction) -> Result<(), String> {
    let valid = matches!(
        (state, action),
        (
            RecordingState::Idle | RecordingState::Finished | RecordingState::Failed,
            RecordAction::Start
        ) | (
            RecordingState::Recording,
            RecordAction::Pause | RecordAction::Finish
        ) | (
            RecordingState::Paused,
            RecordAction::Resume | RecordAction::Finish
        ) | (RecordingState::Finished, RecordAction::Play)
    );
    valid.then_some(()).ok_or_else(|| {
        format!(
            "cannot {} while recorder is {}",
            action.label(),
            state.label()
        )
    })
}

impl RecordAction {
    fn label(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::Finish => "finish",
            Self::Play => "play",
        }
    }
}

impl RecordingState {
    fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Recording => "recording",
            Self::Paused => "paused",
            Self::Finished => "finished",
            Self::Failed => "failed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_complete_recording_lifecycle() {
        assert!(ensure_transition(RecordingState::Idle, RecordAction::Start).is_ok());
        assert!(ensure_transition(RecordingState::Recording, RecordAction::Pause).is_ok());
        assert!(ensure_transition(RecordingState::Paused, RecordAction::Resume).is_ok());
        assert!(ensure_transition(RecordingState::Paused, RecordAction::Finish).is_ok());
        assert!(ensure_transition(RecordingState::Finished, RecordAction::Play).is_ok());
    }

    #[test]
    fn rejects_invalid_recording_controls() {
        assert!(ensure_transition(RecordingState::Idle, RecordAction::Pause).is_err());
        assert!(ensure_transition(RecordingState::Recording, RecordAction::Play).is_err());
        assert!(ensure_transition(RecordingState::Finished, RecordAction::Resume).is_err());
    }

    #[test]
    fn recording_filename_uses_date_and_next_number() {
        let media = std::env::temp_dir().join(format!("friday-path-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&media);
        std::fs::create_dir_all(&media).unwrap();
        std::fs::write(media.join("20260726_001.wav"), []).unwrap();
        std::fs::write(media.join("20260726_003.wav"), []).unwrap();
        assert_eq!(
            next_recording_path(&media, "20260726").unwrap(),
            media.join("20260726_004.wav")
        );
        std::fs::remove_dir_all(media).unwrap();
    }

    #[test]
    #[ignore = "requires a system microphone"]
    fn records_pauses_resumes_and_finishes_wav() {
        use std::{thread, time::Duration};

        let mut recorder = FridayRecorder::new();
        recorder.start().unwrap();
        thread::sleep(Duration::from_millis(300));
        recorder.pause().unwrap();
        thread::sleep(Duration::from_millis(500));
        recorder.resume().unwrap();
        thread::sleep(Duration::from_millis(300));
        recorder.finish().unwrap();

        let reader = hound::WavReader::open(recorder.path().unwrap()).unwrap();
        let seconds = reader.duration() as f64 / reader.spec().sample_rate as f64;
        assert!((0.3..0.9).contains(&seconds), "recorded {seconds:.2} s");
    }
}
