//! Decode and play a `.scd` sound file natively.

use std::io::Cursor;
use std::time::Duration;

use ironworks::file::File;
use ironworks::file::scd::SoundContainer;
use viewer::audio::{Player, decode};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: play <file.scd> [seconds]");
    let seconds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(30);

    let container = SoundContainer::read(Cursor::new(std::fs::read(&path)?))?;
    let entry = container
        .entries()
        .first()
        .expect("file has no audio streams");
    let decoded = decode(entry)?;
    println!(
        "{:?} {}ch {}Hz {} frames, loop {:?}..{:?}",
        entry.format(),
        decoded.channels,
        decoded.sample_rate,
        decoded.samples.len() / decoded.channels as usize,
        decoded.loop_start,
        decoded.loop_end,
    );

    let mut player = Player::new()?;
    player.play(decoded)?;
    println!("playing for {seconds}s...");
    std::thread::sleep(Duration::from_secs(seconds));
    Ok(())
}
