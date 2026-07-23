use std::cell::RefCell;
use std::rc::Rc;

use anyhow::{Result, anyhow};
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use web_sys::{
    AudioBuffer, AudioBufferSourceNode, AudioContext, AudioContextOptions, AudioContextState,
    GainNode, MediaMetadata, MediaPositionState, MediaSession, MediaSessionAction,
    MediaSessionPlaybackState,
};

use super::Decoded;

type SourceCell = Rc<RefCell<Option<AudioBufferSourceNode>>>;

/// Web Audio output
pub struct Player {
    context: AudioContext,
    gain: GainNode,
    source: SourceCell,
    buffer: Option<AudioBuffer>,
    loop_region: Option<(f64, f64)>,
    duration: f64,
    started_at: f64,
    _handlers: Vec<Closure<dyn FnMut()>>,
}

impl Player {
    pub fn new() -> Result<Self> {
        // 44.1 kHz (the BGM rate) so Web Audio doesn't resample buffers.
        let options = AudioContextOptions::new();
        options.set_sample_rate(44_100.0);
        let context =
            AudioContext::new_with_context_options(&options).map_err(js("AudioContext"))?;
        let gain = context.create_gain().map_err(js("create_gain"))?;
        gain.connect_with_audio_node(&context.destination())
            .map_err(js("connect gain"))?;

        let source: SourceCell = Rc::new(RefCell::new(None));
        let handlers = register_media_session(&context, &source);
        Ok(Self {
            context,
            gain,
            source,
            buffer: None,
            loop_region: None,
            duration: 0.0,
            started_at: 0.0,
            _handlers: handlers,
        })
    }

    pub fn play(&mut self, audio: Decoded) -> Result<()> {
        self.stop();

        let channels = audio.channels as usize;
        let frames = audio.samples.len() / channels;
        let rate = f64::from(audio.sample_rate);
        let buffer = self
            .context
            .create_buffer(
                audio.channels as u32,
                frames as u32,
                audio.sample_rate as f32,
            )
            .map_err(js("create_buffer"))?;

        let mut channel = vec![0f32; frames];
        for ch in 0..channels {
            for (frame, slot) in channel.iter_mut().enumerate() {
                *slot = audio.samples[frame * channels + ch];
            }
            buffer
                .copy_to_channel(&channel, ch as i32)
                .map_err(js("copy_to_channel"))?;
        }

        self.duration = frames as f64 / rate;
        self.loop_region = audio
            .loop_start
            .zip(audio.loop_end)
            .map(|(start, end)| (f64::from(start) / rate, f64::from(end) / rate));
        self.buffer = Some(buffer);

        self.start_source(0.0)?;
        // play() runs in a user gesture, so resume is allowed.
        let _ = self.context.resume();
        set_playback_state(MediaSessionPlaybackState::Playing);
        self.publish_position();
        Ok(())
    }

    pub fn seek(&mut self, seconds: f64) {
        if self.buffer.is_none() {
            return;
        }
        let seconds = seconds.clamp(0.0, self.duration);
        if let Some(source) = self.source.borrow_mut().take() {
            #[allow(deprecated)]
            let _ = source.stop();
        }
        let _ = self.start_source(seconds);
        self.publish_position();
    }

    fn start_source(&mut self, offset: f64) -> Result<()> {
        let Some(buffer) = &self.buffer else {
            return Ok(());
        };
        let source = self
            .context
            .create_buffer_source()
            .map_err(js("create_buffer_source"))?;
        source.set_buffer(Some(buffer));
        if let Some((start, end)) = self.loop_region {
            source.set_loop(true);
            source.set_loop_start(start);
            source.set_loop_end(end);
        }
        source
            .connect_with_audio_node(&self.gain)
            .map_err(js("connect source"))?;
        source
            .start_with_when_and_grain_offset(0.0, offset)
            .map_err(js("start"))?;
        self.started_at = self.context.current_time() - offset;
        *self.source.borrow_mut() = Some(source);
        Ok(())
    }

    pub fn position(&self) -> f64 {
        if self.buffer.is_none() {
            return 0.0;
        }
        let elapsed = self.context.current_time() - self.started_at;
        match self.loop_region {
            Some((start, end)) if elapsed >= end => start + (elapsed - start) % (end - start),
            _ => elapsed.clamp(0.0, self.duration),
        }
    }

    pub fn duration(&self) -> f64 {
        self.duration
    }

    pub fn set_metadata(&self, title: &str) {
        if let Some(session) = media_session()
            && let Ok(metadata) = MediaMetadata::new()
        {
            metadata.set_title(title);
            session.set_metadata(Some(&metadata));
        }
    }

    pub fn pause(&self) {
        let _ = self.context.suspend();
        set_playback_state(MediaSessionPlaybackState::Paused);
    }

    pub fn resume(&self) {
        let _ = self.context.resume();
        set_playback_state(MediaSessionPlaybackState::Playing);
        self.publish_position();
    }

    pub fn stop(&mut self) {
        if let Some(source) = self.source.borrow_mut().take() {
            #[allow(deprecated)]
            let _ = source.stop();
        }
        self.buffer = None;
        self.loop_region = None;
        self.duration = 0.0;
        set_playback_state(MediaSessionPlaybackState::None);
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.gain.gain().set_value(volume);
    }

    pub fn unlock(&self) {
        let _ = self.context.resume();
    }

    pub fn is_playing(&self) -> bool {
        self.source.borrow().is_some() && self.context.state() == AudioContextState::Running
    }

    /// Update the lock-screen scrubber; the OS extrapolates from here.
    fn publish_position(&self) {
        if self.duration <= 0.0 {
            return;
        }
        if let Some(session) = media_session() {
            let state = MediaPositionState::new();
            state.set_duration(self.duration);
            state.set_playback_rate(1.0);
            state.set_position(self.position().clamp(0.0, self.duration));
            session.set_position_state_with_state(&state);
        }
    }
}

fn media_session() -> Option<MediaSession> {
    Some(web_sys::window()?.navigator().media_session())
}

fn set_playback_state(state: MediaSessionPlaybackState) {
    if let Some(session) = media_session() {
        session.set_playback_state(state);
    }
}

/// Wire lock-screen / media-key handlers. The returned closures must be kept alive for the
/// handlers to stay attached.
fn register_media_session(
    context: &AudioContext,
    source: &SourceCell,
) -> Vec<Closure<dyn FnMut()>> {
    let Some(session) = media_session() else {
        return Vec::new();
    };

    let play = {
        let context = context.clone();
        Closure::<dyn FnMut()>::new(move || {
            let _ = context.resume();
            set_playback_state(MediaSessionPlaybackState::Playing);
        })
    };
    let pause = {
        let context = context.clone();
        Closure::<dyn FnMut()>::new(move || {
            let _ = context.suspend();
            set_playback_state(MediaSessionPlaybackState::Paused);
        })
    };
    let stop = {
        let source = source.clone();
        Closure::<dyn FnMut()>::new(move || {
            if let Some(source) = source.borrow_mut().take() {
                #[allow(deprecated)]
                let _ = source.stop();
            }
            set_playback_state(MediaSessionPlaybackState::None);
        })
    };

    session.set_action_handler(
        MediaSessionAction::Play,
        Some(play.as_ref().unchecked_ref()),
    );
    session.set_action_handler(
        MediaSessionAction::Pause,
        Some(pause.as_ref().unchecked_ref()),
    );
    session.set_action_handler(
        MediaSessionAction::Stop,
        Some(stop.as_ref().unchecked_ref()),
    );

    vec![play, pause, stop]
}

fn js(context: &'static str) -> impl Fn(JsValue) -> anyhow::Error {
    move |error| anyhow!("{context}: {error:?}")
}
