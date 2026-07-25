use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::Result;
use rodio::{ChannelCount, DeviceSinkBuilder, MixerDeviceSink, SampleRate, Source};

use super::Decoded;

/// Native audio output via rodio
pub struct Player {
    device: MixerDeviceSink,
    sink: Option<rodio::Player>,
    audio: Option<Arc<Decoded>>,
    position: Arc<AtomicU64>,
    volume: f32,
}

impl Player {
    pub fn new() -> Result<Self> {
        Ok(Self {
            device: DeviceSinkBuilder::open_default_sink()?,
            sink: None,
            audio: None,
            position: Arc::new(AtomicU64::new(0)),
            volume: 1.0,
        })
    }

    pub fn play(&mut self, audio: Decoded) -> Result<()> {
        self.audio = Some(Arc::new(audio));
        self.start_from(0)
    }

    pub fn seek(&mut self, seconds: f64) {
        if let Some(audio) = &self.audio {
            let frame = (seconds.max(0.0) * f64::from(audio.sample_rate)) as u64;
            let _ = self.start_from(frame);
        }
    }

    fn start_from(&mut self, frame: u64) -> Result<()> {
        let Some(audio) = self.audio.clone() else {
            return Ok(());
        };
        self.position.store(frame, Ordering::Relaxed);
        let sink = rodio::Player::connect_new(self.device.mixer());
        sink.set_volume(self.volume);
        sink.append(LoopingSource::new(audio, frame, self.position.clone()));
        self.sink = Some(sink);
        Ok(())
    }

    pub fn position(&self) -> f64 {
        match &self.audio {
            Some(audio) => {
                self.position.load(Ordering::Relaxed) as f64 / f64::from(audio.sample_rate)
            }
            None => 0.0,
        }
    }

    pub fn duration(&self) -> f64 {
        match &self.audio {
            Some(audio) => {
                (audio.samples.len() / audio.channels as usize) as f64
                    / f64::from(audio.sample_rate)
            }
            None => 0.0,
        }
    }

    pub fn pause(&self) {
        if let Some(sink) = &self.sink {
            sink.pause();
        }
    }

    pub fn resume(&self) {
        if let Some(sink) = &self.sink {
            sink.play();
        }
    }

    pub fn stop(&mut self) {
        self.sink = None;
        self.audio = None;
        self.position.store(0, Ordering::Relaxed);
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume;
        if let Some(sink) = &self.sink {
            sink.set_volume(volume);
        }
    }

    /// No-op on native; OS media controls are a later addition.
    pub fn set_metadata(&self, _title: &str) {}

    /// No-op on native; on web this resumes the audio context in a user gesture.
    pub fn unlock(&self) {}

    /// No spectrum analyser on the native backend; the visualizer is web-only.
    pub fn spectrum(&self, out: &mut [u8]) {
        out.fill(0);
    }

    /// No OS media controls on the native backend (souvlaki is deferred).
    pub fn take_media_action(&mut self) {}

    pub fn is_playing(&self) -> bool {
        self.sink
            .as_ref()
            .is_some_and(|sink| !sink.empty() && !sink.is_paused())
    }
}

struct LoopingSource {
    audio: Arc<Decoded>,
    index: usize,
    loop_region: Option<(usize, usize)>,
    channels: usize,
    position: Arc<AtomicU64>,
}

impl LoopingSource {
    fn new(audio: Arc<Decoded>, start_frame: u64, position: Arc<AtomicU64>) -> Self {
        let channels = audio.channels as usize;
        let loop_region = audio
            .loop_start
            .zip(audio.loop_end)
            .map(|(start, end)| (start as usize * channels, end as usize * channels));
        Self {
            index: start_frame as usize * channels,
            loop_region,
            channels,
            position,
            audio,
        }
    }
}

impl Iterator for LoopingSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if let Some((start, end)) = self.loop_region
            && self.index >= end
        {
            self.index = start;
        }
        let sample = self.audio.samples.get(self.index).copied();
        if sample.is_some() {
            self.index += 1;
            self.position
                .store((self.index / self.channels) as u64, Ordering::Relaxed);
        }
        sample
    }
}

impl Source for LoopingSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        ChannelCount::new(self.audio.channels).unwrap()
    }

    fn sample_rate(&self) -> SampleRate {
        SampleRate::new(self.audio.sample_rate).unwrap()
    }

    fn total_duration(&self) -> Option<Duration> {
        if self.loop_region.is_some() {
            return None;
        }
        let frames = self.audio.samples.len() as f64 / f64::from(self.audio.channels);
        Some(Duration::from_secs_f64(
            frames / f64::from(self.audio.sample_rate),
        ))
    }
}
