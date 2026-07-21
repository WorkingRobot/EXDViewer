use std::io::Cursor;

use anyhow::{Result, anyhow};
use ironworks::file::scd::{Codec, SoundEntry};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::{MetadataOptions, MetadataRevision};
use symphonia::core::probe::Hint;

/// Decoded interleaved f32 PCM plus its loop region.
pub struct Decoded {
    pub samples: Vec<f32>,
    pub channels: u16,
    pub sample_rate: u32,
    /// Per-channel frame indices; `None` if the track does not loop.
    pub loop_start: Option<u32>,
    pub loop_end: Option<u32>,
}

/// Decode a BGM sound entry to interleaved PCM.
pub fn decode(entry: &SoundEntry) -> Result<Decoded> {
    match entry.format() {
        Codec::OggVorbis => decode_ogg(entry.data()),
        Codec::Hca => decode_hca(entry.data()),
        other => Err(anyhow!("unsupported audio codec {other:?}")),
    }
}

/// OggVorbis via symphonia. Loop points come from the `LoopStart`/`LoopEnd` Vorbis comments,
/// as the game uses; the SCD byte offsets are ignored.
fn decode_ogg(data: &[u8]) -> Result<Decoded> {
    let stream = MediaSourceStream::new(Box::new(Cursor::new(data.to_vec())), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("ogg");
    let mut probed = symphonia::default::get_probe().format(
        &hint,
        stream,
        &FormatOptions {
            enable_gapless: true,
            ..Default::default()
        },
        &MetadataOptions::default(),
    )?;
    let mut format = probed.format;

    let (mut loop_start, mut loop_end) = (None, None);
    let mut scan = |revision: &MetadataRevision| {
        for tag in revision.tags() {
            match tag.key.to_ascii_uppercase().as_str() {
                "LOOPSTART" => loop_start = tag.value.to_string().parse().ok(),
                "LOOPEND" => loop_end = tag.value.to_string().parse().ok(),
                _ => {}
            }
        }
    };
    if let Some(revision) = probed.metadata.get().as_ref().and_then(|meta| meta.current()) {
        scan(revision);
    }
    if let Some(revision) = format.metadata().current() {
        scan(revision);
    }

    let track = format
        .default_track()
        .ok_or_else(|| anyhow!("ogg has no default track"))?;
    let channels = track
        .codec_params
        .channels
        .ok_or_else(|| anyhow!("ogg track has no channel layout"))?
        .count() as u16;
    let sample_rate = track
        .codec_params
        .sample_rate
        .ok_or_else(|| anyhow!("ogg track has no sample rate"))?;
    let track_id = track.id;
    let mut decoder =
        symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;

    let mut samples = Vec::new();
    let mut buffer: Option<SampleBuffer<f32>> = None;
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(_)) => break, // end of stream
            Err(error) => return Err(error.into()),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(audio) => {
                let buffer = buffer
                    .get_or_insert_with(|| SampleBuffer::new(audio.capacity() as u64, *audio.spec()));
                buffer.copy_interleaved_ref(audio);
                samples.extend_from_slice(buffer.samples());
            }
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(error) => return Err(error.into()),
        }
    }

    Ok(Decoded {
        samples,
        channels,
        sample_rate,
        loop_start,
        loop_end,
    })
}

/// HCA via cridecoder. `decode_all` output shares the header's loop-block timeline (delay and
/// padding already trimmed); the loop maths mirror vgmstream.
fn decode_hca(data: &[u8]) -> Result<Decoded> {
    let mut decoder = cridecoder::HcaDecoder::from_reader(Cursor::new(data.to_vec()))
        .map_err(|error| anyhow!("hca: {error:?}"))?;
    let info = decoder.info().clone();
    let samples = decoder
        .decode_all()
        .map_err(|error| anyhow!("hca decode: {error:?}"))?;

    let (loop_start, loop_end) = if info.loop_enabled {
        let per_block = info.samples_per_block as u32;
        let delay = info.encoder_delay;
        (
            Some(info.loop_start_block * per_block - delay + info.loop_start_delay),
            Some((info.loop_end_block + 1) * per_block - delay - info.loop_end_padding),
        )
    } else {
        (None, None)
    };

    Ok(Decoded {
        samples,
        channels: info.channel_count as u16,
        sample_rate: info.sampling_rate,
        loop_start,
        loop_end,
    })
}
