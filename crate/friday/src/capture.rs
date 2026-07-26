//! Platform audio capture backend used by [`crate::FridayRecorder`].

use std::path::Path;

fn rms_level(square_sum: f64, count: usize) -> f32 {
    if count == 0 {
        return 0.0;
    }
    ((square_sum / count as f64).sqrt() / 32_768.0 * 8.0).clamp(0.0, 1.0) as f32
}

#[cfg(target_os = "windows")]
mod platform {
    use std::{
        fs::File,
        io::BufWriter,
        path::Path,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicU32, Ordering},
        },
    };

    use cpal::{
        FromSample, I24, Sample, SampleFormat, SizedSample, Stream,
        traits::{DeviceTrait, HostTrait, StreamTrait},
    };
    use hound::{SampleFormat as WavSampleFormat, WavSpec, WavWriter};

    struct WriterState {
        wav: Mutex<Option<WavWriter<BufWriter<File>>>>,
        level: AtomicU32,
    }

    type Writer = Arc<WriterState>;

    pub(crate) struct Capture {
        stream: Stream,
        writing: Arc<AtomicBool>,
        writer: Writer,
        error: Arc<Mutex<Option<String>>>,
    }

    impl Capture {
        pub(crate) fn start(path: &Path) -> Result<Self, String> {
            let device = cpal::default_host()
                .default_input_device()
                .ok_or_else(|| "no input audio device found".to_string())?;
            let supported = device
                .default_input_config()
                .map_err(|error| format!("cannot read microphone config: {error}"))?;
            let sample_format = supported.sample_format();
            let config = supported.config();
            let spec = WavSpec {
                channels: config.channels,
                sample_rate: config.sample_rate.0,
                bits_per_sample: 16,
                sample_format: WavSampleFormat::Int,
            };
            let writer = Arc::new(WriterState {
                wav: Mutex::new(Some(
                    WavWriter::create(path, spec)
                        .map_err(|error| format!("cannot create recording: {error}"))?,
                )),
                level: AtomicU32::new(0.0_f32.to_bits()),
            });
            let writing = Arc::new(AtomicBool::new(true));
            let error = Arc::new(Mutex::new(None));
            let stream = build_stream(
                &device,
                &config,
                sample_format,
                Arc::clone(&writer),
                Arc::clone(&writing),
                Arc::clone(&error),
            )?;
            stream
                .play()
                .map_err(|error| format!("cannot start microphone: {error}"))?;
            Ok(Self {
                stream,
                writing,
                writer,
                error,
            })
        }

        pub(crate) fn pause(&mut self) -> Result<(), String> {
            self.writing.store(false, Ordering::Release);
            self.writer
                .level
                .store(0.0_f32.to_bits(), Ordering::Relaxed);
            Ok(())
        }

        pub(crate) fn resume(&mut self) -> Result<(), String> {
            self.writing.store(true, Ordering::Release);
            Ok(())
        }

        pub(crate) fn finish(self) -> Result<(), String> {
            self.writing.store(false, Ordering::Release);
            drop(self.stream);
            let error = self.error.lock().ok().and_then(|mut error| error.take());
            let writer = self
                .writer
                .wav
                .lock()
                .map_err(|_| "recording writer lock poisoned".to_string())?
                .take();
            if let Some(writer) = writer {
                writer
                    .finalize()
                    .map_err(|error| format!("cannot finish recording: {error}"))?;
            }
            error.map_or(Ok(()), Err)
        }

        pub(crate) fn poll_error(&mut self) -> Option<String> {
            self.error.lock().ok().and_then(|mut error| error.take())
        }

        pub(crate) fn level(&self) -> f32 {
            f32::from_bits(self.writer.level.load(Ordering::Relaxed))
        }
    }

    fn build_stream(
        device: &cpal::Device,
        config: &cpal::StreamConfig,
        format: SampleFormat,
        writer: Writer,
        writing: Arc<AtomicBool>,
        error: Arc<Mutex<Option<String>>>,
    ) -> Result<Stream, String> {
        match format {
            SampleFormat::I8 => build_typed_stream::<i8>(device, config, writer, writing, error),
            SampleFormat::I16 => build_typed_stream::<i16>(device, config, writer, writing, error),
            SampleFormat::I24 => build_typed_stream::<I24>(device, config, writer, writing, error),
            SampleFormat::I32 => build_typed_stream::<i32>(device, config, writer, writing, error),
            SampleFormat::I64 => build_typed_stream::<i64>(device, config, writer, writing, error),
            SampleFormat::U8 => build_typed_stream::<u8>(device, config, writer, writing, error),
            SampleFormat::U16 => build_typed_stream::<u16>(device, config, writer, writing, error),
            SampleFormat::U32 => build_typed_stream::<u32>(device, config, writer, writing, error),
            SampleFormat::U64 => build_typed_stream::<u64>(device, config, writer, writing, error),
            SampleFormat::F32 => build_typed_stream::<f32>(device, config, writer, writing, error),
            SampleFormat::F64 => build_typed_stream::<f64>(device, config, writer, writing, error),
            _ => Err(format!("unsupported microphone sample format: {format:?}")),
        }
    }

    fn build_typed_stream<T>(
        device: &cpal::Device,
        config: &cpal::StreamConfig,
        writer: Writer,
        writing: Arc<AtomicBool>,
        error: Arc<Mutex<Option<String>>>,
    ) -> Result<Stream, String>
    where
        T: Sample + SizedSample,
        i16: FromSample<T>,
    {
        let stream_error = Arc::clone(&error);
        device
            .build_input_stream(
                config,
                move |data: &[T], _| write_samples(data, &writer, &writing, &error),
                move |cause| set_error(&stream_error, format!("microphone failed: {cause}")),
                None,
            )
            .map_err(|error| format!("cannot open microphone: {error}"))
    }

    fn write_samples<T>(
        data: &[T],
        writer: &Writer,
        writing: &AtomicBool,
        error: &Mutex<Option<String>>,
    ) where
        T: Sample + Copy,
        i16: FromSample<T>,
    {
        if !writing.load(Ordering::Acquire) {
            return;
        }
        let Ok(mut guard) = writer.wav.lock() else {
            set_error(error, "recording writer lock poisoned".to_string());
            return;
        };
        let Some(wav) = guard.as_mut() else {
            return;
        };
        let mut square_sum = 0.0;
        let mut count = 0;
        for &sample in data {
            let sample = i16::from_sample(sample);
            square_sum += f64::from(sample).powi(2);
            count += 1;
            if let Err(cause) = wav.write_sample(sample) {
                writing.store(false, Ordering::Release);
                set_error(error, format!("cannot write recording: {cause}"));
                break;
            }
        }
        writer.level.store(
            super::rms_level(square_sum, count).to_bits(),
            Ordering::Relaxed,
        );
    }

    fn set_error(slot: &Mutex<Option<String>>, message: String) {
        if let Ok(mut slot) = slot.lock() {
            *slot = Some(message);
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::{
        fs::File,
        io::{Read, Seek, SeekFrom},
        path::{Path, PathBuf},
        process::{Child, Command, Stdio},
        thread,
        time::{Duration, Instant},
    };

    use hound::{SampleFormat, WavReader, WavSpec, WavWriter};

    const STOP_GRACE: Duration = Duration::from_millis(500);

    pub(crate) struct Capture {
        program: String,
        path: PathBuf,
        segments: Vec<PathBuf>,
        child: Option<Child>,
    }

    impl Capture {
        pub(crate) fn start(path: &Path) -> Result<Self, String> {
            let mut capture = Self {
                program: std::env::var("FRIDAY_ARECORD").unwrap_or_else(|_| "arecord".into()),
                path: path.to_owned(),
                segments: Vec::new(),
                child: None,
            };
            capture.spawn_segment()?;
            Ok(capture)
        }

        pub(crate) fn pause(&mut self) -> Result<(), String> {
            self.stop_segment()
        }

        pub(crate) fn resume(&mut self) -> Result<(), String> {
            self.spawn_segment()
        }

        pub(crate) fn finish(mut self) -> Result<(), String> {
            self.stop_segment()?;
            let spec = WavSpec {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: SampleFormat::Int,
            };
            let mut output = WavWriter::create(&self.path, spec)
                .map_err(|error| format!("cannot create recording: {error}"))?;
            for path in &self.segments {
                let mut input = WavReader::open(path)
                    .map_err(|error| format!("cannot read recording segment: {error}"))?;
                for sample in input.samples::<i16>() {
                    output
                        .write_sample(
                            sample.map_err(|error| {
                                format!("cannot read recording sample: {error}")
                            })?,
                        )
                        .map_err(|error| format!("cannot write recording: {error}"))?;
                }
            }
            output
                .finalize()
                .map_err(|error| format!("cannot finish recording: {error}"))
        }

        pub(crate) fn poll_error(&mut self) -> Option<String> {
            let result = self.child.as_mut()?.try_wait();
            match result {
                Ok(Some(status)) => {
                    self.child.take();
                    Some(format!("audio recorder exited unexpectedly: {status}"))
                }
                Ok(None) => None,
                Err(error) => Some(format!("cannot query audio recorder: {error}")),
            }
        }

        pub(crate) fn level(&self) -> f32 {
            if self.child.is_none() {
                return 0.0;
            }
            self.segments
                .last()
                .and_then(|path| recent_level(path))
                .unwrap_or_default()
        }

        fn spawn_segment(&mut self) -> Result<(), String> {
            use std::os::unix::process::CommandExt;

            let path = self
                .path
                .with_extension(format!("part-{}.wav", self.segments.len()));
            let mut command = Command::new(&self.program);
            command
                .args(["-q", "-t", "wav", "-f", "S16_LE", "-r", "16000", "-c", "1"])
                .arg(&path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .process_group(0);
            self.child = Some(
                command
                    .spawn()
                    .map_err(|error| format!("cannot start {}: {error}", self.program))?,
            );
            self.segments.push(path);
            Ok(())
        }

        fn stop_segment(&mut self) -> Result<(), String> {
            let Some(child) = self.child.as_mut() else {
                return Ok(());
            };
            if child
                .try_wait()
                .map_err(|error| format!("cannot query audio recorder: {error}"))?
                .is_some()
            {
                self.child.take();
                return Err("audio recorder stopped unexpectedly".to_string());
            }
            signal(child.id(), libc::SIGINT)?;

            let deadline = Instant::now() + STOP_GRACE;
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => {
                        self.child.take();
                        return Ok(());
                    }
                    Ok(None) if Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Ok(None) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        self.child.take();
                        return Err("audio recorder did not stop cleanly".to_string());
                    }
                    Err(error) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        self.child.take();
                        return Err(format!("cannot finish recording: {error}"));
                    }
                }
            }
        }
    }

    impl Drop for Capture {
        fn drop(&mut self) {
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            for path in &self.segments {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    fn recent_level(path: &Path) -> Option<f32> {
        let mut file = File::open(path).ok()?;
        let length = file.metadata().ok()?.len();
        let bytes = length.saturating_sub(44).min(4096) as usize & !1;
        if bytes == 0 {
            return Some(0.0);
        }
        file.seek(SeekFrom::End(-(bytes as i64))).ok()?;
        let mut buffer = vec![0; bytes];
        file.read_exact(&mut buffer).ok()?;
        let mut square_sum = 0.0;
        let mut count = 0;
        for sample in buffer.chunks_exact(2) {
            let sample = i16::from_le_bytes([sample[0], sample[1]]);
            square_sum += f64::from(sample).powi(2);
            count += 1;
        }
        Some(super::rms_level(square_sum, count))
    }

    fn signal(pid: u32, signal: libc::c_int) -> Result<(), String> {
        // The recorder owns a process group so Stop cannot affect the GUI.
        let result = unsafe { libc::kill(-(pid as libc::pid_t), signal) };
        if result == 0 {
            Ok(())
        } else {
            Err(format!(
                "cannot stop recording: {}",
                std::io::Error::last_os_error()
            ))
        }
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
mod platform {
    use std::path::Path;

    pub(crate) struct Capture;

    impl Capture {
        pub(crate) fn start(_path: &Path) -> Result<Self, String> {
            Err("audio recording is supported on Windows and Linux".to_string())
        }

        pub(crate) fn pause(&mut self) -> Result<(), String> {
            Ok(())
        }

        pub(crate) fn resume(&mut self) -> Result<(), String> {
            Ok(())
        }

        pub(crate) fn finish(self) -> Result<(), String> {
            Ok(())
        }

        pub(crate) fn poll_error(&mut self) -> Option<String> {
            None
        }

        pub(crate) fn level(&self) -> f32 {
            0.0
        }
    }
}

pub(crate) use platform::Capture;

pub(crate) fn start(path: &Path) -> Result<Capture, String> {
    Capture::start(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rms_meter_tracks_silence_and_signal() {
        assert_eq!(rms_level(0.0, 1024), 0.0);
        let sample = 4096.0_f64;
        let level = rms_level(sample * sample * 1024.0, 1024);
        assert!((0.99..=1.0).contains(&level));
    }
}
