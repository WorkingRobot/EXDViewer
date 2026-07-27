use std::cell::RefCell;
use std::rc::Rc;

use anyhow::{Result, anyhow};
use base64::{Engine as _, prelude::BASE64_STANDARD};
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use web_sys::{
    AnalyserNode, AudioBuffer, AudioBufferSourceNode, AudioContext, AudioContextState, GainNode,
    HtmlAudioElement, MediaMetadata, MediaPositionState, MediaSession, MediaSessionAction,
    MediaSessionActionDetails, MediaSessionPlaybackState, js_sys::Reflect,
};

use super::Decoded;

type SourceCell = Rc<RefCell<Option<AudioBufferSourceNode>>>;

type Generation = Rc<std::cell::Cell<u64>>;

#[derive(Clone, Copy)]
enum SeekReq {
    To(f64),
    By(f64),
}
type SeekCell = Rc<RefCell<Option<SeekReq>>>;

pub struct Player {
    context: AudioContext,
    gain: GainNode,
    analyser: AnalyserNode,
    anchor: Option<HtmlAudioElement>,
    source: SourceCell,
    buffer: Option<AudioBuffer>,
    loop_region: Option<(f64, f64)>,
    duration: f64,
    started_at: f64,
    seek_req: SeekCell,
    generation: Generation,
    _handlers: Vec<Closure<dyn FnMut()>>,
    _seek_handlers: Vec<Closure<dyn FnMut(MediaSessionActionDetails)>>,
    _ended_handler: Option<Closure<dyn FnMut()>>,
}

impl Player {
    pub fn new() -> Result<Self> {
        let context = AudioContext::new().map_err(js("AudioContext"))?;
        let gain = context.create_gain().map_err(js("create_gain"))?;
        let analyser = context.create_analyser().map_err(js("create_analyser"))?;
        analyser.set_fft_size(8192);
        analyser.set_smoothing_time_constant(0.6);
        analyser.set_min_decibels(-85.0);
        analyser.set_max_decibels(-25.0);
        gain.connect_with_audio_node(&analyser)
            .map_err(js("connect gain"))?;
        analyser
            .connect_with_audio_node(&context.destination())
            .map_err(js("connect analyser"))?;

        let mut handlers = Vec::new();
        let anchor = match focus_token() {
            Ok(anchor) => {
                handlers.extend(bind_anchor(&context, &anchor));
                Some(anchor)
            }
            Err(error) => {
                log::warn!("audio-focus token unavailable, OS controls limited: {error}");
                None
            }
        };

        let source: SourceCell = Rc::new(RefCell::new(None));
        handlers.extend(register_media_session(&context, &source, anchor.as_ref()));

        let seek_req: SeekCell = Rc::new(RefCell::new(None));
        let seek_handlers = register_seek_handlers(&seek_req);

        Ok(Self {
            context,
            gain,
            analyser,
            anchor,
            source,
            buffer: None,
            loop_region: None,
            duration: 0.0,
            started_at: 0.0,
            seek_req,
            generation: Rc::new(std::cell::Cell::new(0)),
            _handlers: handlers,
            _seek_handlers: seek_handlers,
            _ended_handler: None,
        })
    }

    pub fn spectrum(&self, out: &mut [u8]) {
        self.analyser.get_byte_frequency_data(out);
    }

    pub fn take_media_action(&mut self) {
        let request = self.seek_req.borrow_mut().take();
        match request {
            Some(SeekReq::To(time)) => self.seek(time),
            Some(SeekReq::By(delta)) => {
                let target = self.position() + delta;
                self.seek(target);
            }
            None => {}
        }
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
        let _ = self.context.resume();
        if let Some(anchor) = &self.anchor {
            let _ = anchor.play();
        }
        set_playback_state(MediaSessionPlaybackState::Playing);
        self.publish_position();
        Ok(())
    }

    pub fn seek(&mut self, seconds: f64) {
        if self.buffer.is_none() {
            return;
        }
        let seconds = seconds.clamp(0.0, self.duration);
        self.discard_source();
        let _ = self.start_source(seconds);
        self.publish_position();
    }

    fn discard_source(&mut self) {
        self.generation.set(self.generation.get().wrapping_add(1));
        if let Some(source) = self.source.borrow_mut().take() {
            #[allow(deprecated)]
            {
                source.set_onended(None);
                let _ = source.stop();
            }
        }
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

        if self.loop_region.is_none() {
            let generation = self.generation.clone();
            let attached_at = generation.get();
            let cell = self.source.clone();
            let anchor = self.anchor.clone();
            let handler = Closure::<dyn FnMut()>::new(move || {
                if generation.get() != attached_at {
                    return;
                }
                cell.borrow_mut().take();
                if let Some(anchor) = &anchor {
                    let _ = anchor.pause();
                }
                set_playback_state(MediaSessionPlaybackState::None);
            });
            #[allow(deprecated)]
            source.set_onended(Some(handler.as_ref().unchecked_ref()));
            self._ended_handler = Some(handler);
        }

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
        if let Some(anchor) = &self.anchor {
            let _ = anchor.pause();
        }
        let _ = self.context.suspend();
        set_playback_state(MediaSessionPlaybackState::Paused);
    }

    pub fn resume(&self) {
        if let Some(anchor) = &self.anchor {
            let _ = anchor.play();
        }
        let _ = self.context.resume();
        set_playback_state(MediaSessionPlaybackState::Playing);
        self.publish_position();
    }

    pub fn stop(&mut self) {
        self.discard_source();
        if let Some(anchor) = &self.anchor {
            let _ = anchor.pause();
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

/// Wire lock-screen / media-key handlers. When the focus token exists they drive it (its events
/// then sync the context); otherwise they drive the context directly. Returned closures must be
/// kept alive for the handlers to stay attached.
fn register_media_session(
    context: &AudioContext,
    source: &SourceCell,
    anchor: Option<&HtmlAudioElement>,
) -> Vec<Closure<dyn FnMut()>> {
    let Some(session) = media_session() else {
        return Vec::new();
    };

    let play = {
        let context = context.clone();
        let anchor = anchor.cloned();
        Closure::<dyn FnMut()>::new(move || match &anchor {
            Some(anchor) => {
                let _ = anchor.play();
            }
            None => {
                let _ = context.resume();
                set_playback_state(MediaSessionPlaybackState::Playing);
            }
        })
    };
    let pause = {
        let context = context.clone();
        let anchor = anchor.cloned();
        Closure::<dyn FnMut()>::new(move || match &anchor {
            Some(anchor) => {
                let _ = anchor.pause();
            }
            None => {
                let _ = context.suspend();
                set_playback_state(MediaSessionPlaybackState::Paused);
            }
        })
    };
    let stop = {
        let source = source.clone();
        let anchor = anchor.cloned();
        Closure::<dyn FnMut()>::new(move || {
            if let Some(source) = source.borrow_mut().take() {
                #[allow(deprecated)]
                let _ = source.stop();
            }
            if let Some(anchor) = &anchor {
                let _ = anchor.pause();
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

/// Wire the OS scrubber (`seekto`) and skip buttons (`seekbackward`/`seekforward`) to a shared
/// cell drained by [`Player::take_media_action`]. Kept-alive closures are returned.
fn register_seek_handlers(seek: &SeekCell) -> Vec<Closure<dyn FnMut(MediaSessionActionDetails)>> {
    let Some(session) = media_session() else {
        return Vec::new();
    };

    // `MediaSessionActionDetails` is a JS dict web-sys only exposes as setters, so read the
    // incoming fields by reflection.
    fn field(details: &MediaSessionActionDetails, name: &str) -> Option<f64> {
        Reflect::get(details.as_ref(), &JsValue::from_str(name))
            .ok()
            .and_then(|value| value.as_f64())
    }
    let to = {
        let seek = seek.clone();
        Closure::<dyn FnMut(MediaSessionActionDetails)>::new(
            move |details: MediaSessionActionDetails| {
                if let Some(time) = field(&details, "seekTime") {
                    *seek.borrow_mut() = Some(SeekReq::To(time));
                }
            },
        )
    };
    let backward = {
        let seek = seek.clone();
        Closure::<dyn FnMut(MediaSessionActionDetails)>::new(
            move |details: MediaSessionActionDetails| {
                *seek.borrow_mut() =
                    Some(SeekReq::By(-field(&details, "seekOffset").unwrap_or(10.0)));
            },
        )
    };
    let forward = {
        let seek = seek.clone();
        Closure::<dyn FnMut(MediaSessionActionDetails)>::new(
            move |details: MediaSessionActionDetails| {
                *seek.borrow_mut() =
                    Some(SeekReq::By(field(&details, "seekOffset").unwrap_or(10.0)));
            },
        )
    };

    session.set_action_handler(
        MediaSessionAction::Seekto,
        Some(to.as_ref().unchecked_ref()),
    );
    session.set_action_handler(
        MediaSessionAction::Seekbackward,
        Some(backward.as_ref().unchecked_ref()),
    );
    session.set_action_handler(
        MediaSessionAction::Seekforward,
        Some(forward.as_ref().unchecked_ref()),
    );

    vec![to, backward, forward]
}

/// A hidden looping `<audio>` playing inaudible noise. Chrome grants audio focus (pausing other
/// apps) and surfaces OS controls on playback of a real media element — regardless of Media
/// Engagement, unlike pure Web Audio. It is not wired into the graph, so it can't reclock or drift
/// the real audio.
fn focus_token() -> Result<HtmlAudioElement> {
    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| anyhow!("no document"))?;
    let audio = document
        .create_element("audio")
        .map_err(js("create audio element"))?
        .dyn_into::<HtmlAudioElement>()
        .map_err(|_| anyhow!("not an audio element"))?;
    audio.set_src(&format!(
        "data:audio/wav;base64,{}",
        BASE64_STANDARD.encode(focus_token_wav())
    ));
    audio.set_loop(true);
    if let Some(body) = document.body() {
        let _ = body.append_child(audio.unchecked_ref());
    }
    Ok(audio)
}

/// A 10 s mono 8 kHz WAV of a steady ~35 Hz sine at ~-67 dBFS RMS. Chrome on Android defers a
/// media element's exclusive audio-focus request (the one that pauses other apps) until its
/// *measured* output power crosses ~-72.25 dBFS, so the token must be genuinely non-silent: a
/// constant sine holds rock-steady power a few dB over that gate, where noise's fluctuating power
/// can dip under it and read as silent. Imperceptibility comes from the low frequency (small
/// laptop/phone speakers barely reproduce 35 Hz and the ear is very insensitive there), not from
/// going quieter, since below ~-72 dBFS the focus request never fires on Android. 350 whole cycles
/// in 10 s → seamless loop. Longer than 5s so Chrome treats it as persistent media (kGain, pauses
/// others) rather than a transient sound effect.
fn focus_token_wav() -> Vec<u8> {
    const RATE: u32 = 8_000;
    const SECONDS: u32 = 10;
    const FREQ: f64 = 35.0;
    const PEAK: f64 = 21.0; // sine RMS = peak/sqrt(2) → ~-67 dBFS, ~5 dB above the -72.25 dBFS gate
    let frames = (RATE * SECONDS) as usize;
    let data_size = (frames * 2) as u32;

    let mut wav = Vec::with_capacity(44 + frames * 2);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_size).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 2).to_le_bytes()); // byte rate
    wav.extend_from_slice(&2u16.to_le_bytes()); // block align
    wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());

    for i in 0..frames {
        let t = i as f64 / f64::from(RATE);
        let sample = (PEAK * (std::f64::consts::TAU * FREQ * t).sin()) as i16;
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    wav
}

/// Mirror the focus token's play/pause onto the context so it stops advancing whenever the OS, or
/// another app grabbing audio focus, pauses the element.
fn bind_anchor(context: &AudioContext, anchor: &HtmlAudioElement) -> Vec<Closure<dyn FnMut()>> {
    let on_play = {
        let context = context.clone();
        Closure::<dyn FnMut()>::new(move || {
            let _ = context.resume();
            set_playback_state(MediaSessionPlaybackState::Playing);
        })
    };
    let on_pause = {
        let context = context.clone();
        Closure::<dyn FnMut()>::new(move || {
            let _ = context.suspend();
            set_playback_state(MediaSessionPlaybackState::Paused);
        })
    };
    let _ = anchor.add_event_listener_with_callback("play", on_play.as_ref().unchecked_ref());
    let _ = anchor.add_event_listener_with_callback("pause", on_pause.as_ref().unchecked_ref());
    vec![on_play, on_pause]
}

fn js(context: &'static str) -> impl Fn(JsValue) -> anyhow::Error {
    move |error| anyhow!("{context}: {error:?}")
}
