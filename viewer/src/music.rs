//! The `/music` route: a BGM player over [`crate::audio`], listing tracks from the `BGM`
//! sheet annotated with the backend's proxied community song list.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use egui::{Color32, RichText, ScrollArea, Slider};
use ironworks::excel::Language;
use ironworks::file::scd::SoundContainer;
use serde::Deserialize;

use crate::audio::{self, Decoded, Player};
use crate::backend::Backend;
use crate::data::{FileProvider, FileProviderExt};
use crate::excel::base::CachedProvider;
use crate::excel::provider::{ExcelHeader, ExcelProvider, ExcelSheet};
use crate::settings::{BACKEND_CONFIG, InstallLocation};
use crate::utils::{PromiseKind, TrackedPromise, fetch_url_str};

/// Community song metadata keyed by BGM row id, from the backend's `/songs` proxy. Compact
/// keys keep the payload small.
#[derive(Deserialize, Default)]
struct SongInfo {
    #[serde(rename = "t", default)]
    title: String,
    #[serde(rename = "a", default)]
    alt: String,
    #[serde(rename = "s", default)]
    special: String,
    #[serde(rename = "l", default)]
    locations: String,
    #[serde(rename = "i", default)]
    info: String,
    #[serde(rename = "d", default)]
    duration: u32,
}

struct BgmTrack {
    row_id: u32,
    path: String,
}

enum Index {
    Idle,
    Loading(TrackedPromise<Result<Vec<BgmTrack>>>),
    Loaded(Vec<BgmTrack>),
    Failed(String),
}

/// Which track paths this backend serves (web installs are ffxiv-only). Tracks are treated
/// as available until this resolves.
enum Avail {
    Idle,
    Loading(TrackedPromise<Result<HashSet<String>>>),
    Ready(HashSet<String>),
    Failed,
}

/// Song metadata load; resolves to `Done` even on failure (falls back to file names), only
/// attempted against a web backend.
enum Songs {
    Idle,
    Loading(TrackedPromise<Result<HashMap<u32, SongInfo>>>),
    Done,
}

struct Loading {
    row_id: u32,
    name: String,
    path: String,
    promise: TrackedPromise<Result<Decoded>>,
}

struct NowPlaying {
    name: String,
    path: String,
    row_id: u32,
    channels: u16,
    sample_rate: u32,
    looping: bool,
}

pub struct MusicPlayer {
    player: Option<Player>,
    index: Index,
    avail: Avail,
    songs: HashMap<u32, SongInfo>,
    songs_load: Songs,
    loading: Option<Loading>,
    now_playing: Option<NowPlaying>,
    volume: f32,
    search: String,
    show_unavailable: bool,
    /// Set while dragging the seek bar so the thumb follows the pointer, not the advancing
    /// playhead.
    scrub: Option<f64>,
}

impl Default for MusicPlayer {
    fn default() -> Self {
        Self {
            player: None,
            index: Index::Idle,
            avail: Avail::Idle,
            songs: HashMap::new(),
            songs_load: Songs::Idle,
            loading: None,
            now_playing: None,
            volume: 1.0,
            search: String::new(),
            show_unavailable: false,
            scrub: None,
        }
    }
}

impl MusicPlayer {
    pub fn ui(&mut self, ui: &mut egui::Ui, backend: &Backend) {
        let api_url = match BACKEND_CONFIG.get(ui.ctx()) {
            Some(config) => match config.location {
                InstallLocation::Web(url, ..) => Some(url),
                _ => None,
            },
            None => None,
        };
        self.poll(backend, api_url);
        if self.player.as_ref().is_some_and(Player::is_playing) {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        self.player_bar(ui);
        ui.separator();
        self.track_list(ui, backend);
    }

    fn poll(&mut self, backend: &Backend, api_url: Option<String>) {
        if matches!(self.index, Index::Idle) {
            let excel = backend.excel().clone();
            self.index =
                Index::Loading(TrackedPromise::spawn_local(async move { load_index(excel).await }));
        }
        if matches!(&self.index, Index::Loading(p) if p.try_get().is_some()) {
            let Index::Loading(promise) = std::mem::replace(&mut self.index, Index::Idle) else {
                unreachable!()
            };
            self.index = match promise.block_and_take() {
                Ok(tracks) => Index::Loaded(tracks),
                Err(error) => Index::Failed(error.to_string()),
            };
        }

        if matches!(self.songs_load, Songs::Idle)
            && let Some(url) = api_url
        {
            let url = format!("{}/songs/", url.trim_end_matches('/'));
            self.songs_load = Songs::Loading(TrackedPromise::spawn_local(async move {
                Ok(serde_json::from_str(&fetch_url_str(url).await?)?)
            }));
        }
        if matches!(&self.songs_load, Songs::Loading(p) if p.try_get().is_some()) {
            let Songs::Loading(promise) = std::mem::replace(&mut self.songs_load, Songs::Idle) else {
                unreachable!()
            };
            match promise.block_and_take() {
                Ok(songs) => self.songs = songs,
                Err(error) => log::warn!("BGM song list unavailable, using file names: {error}"),
            }
            self.songs_load = Songs::Done;
        }

        if matches!(self.avail, Avail::Idle)
            && let Index::Loaded(tracks) = &self.index
        {
            let files = backend.files().clone();
            let paths: Vec<String> = tracks.iter().map(|track| track.path.clone()).collect();
            self.avail = Avail::Loading(TrackedPromise::spawn_local(async move {
                check_availability(files, paths).await
            }));
        }
        if matches!(&self.avail, Avail::Loading(p) if p.try_get().is_some()) {
            let Avail::Loading(promise) = std::mem::replace(&mut self.avail, Avail::Idle) else {
                unreachable!()
            };
            self.avail = match promise.block_and_take() {
                Ok(available) => Avail::Ready(available),
                Err(_) => Avail::Failed,
            };
        }

        if self.loading.as_ref().is_some_and(|l| l.promise.try_get().is_some()) {
            let Loading {
                row_id,
                name,
                path,
                promise,
            } = self.loading.take().unwrap();
            match promise.block_and_take() {
                Ok(decoded) => self.start(row_id, name, path, decoded),
                Err(error) => log::error!("BGM decode failed: {error}"),
            }
        }
    }

    /// Display title: the community song title, falling back to the file name.
    fn title(&self, row_id: u32, path: &str) -> String {
        self.songs
            .get(&row_id)
            .filter(|song| !song.title.is_empty())
            .map_or_else(|| file_stem(path), |song| song.title.clone())
    }

    /// Resume the audio context in this click gesture (autoplay policy), then fetch+decode.
    fn begin_load(&mut self, backend: &Backend, row_id: u32, path: String) {
        if !self.ensure_player() {
            return;
        }
        if let Some(player) = &mut self.player {
            player.unlock();
            player.stop();
        }
        self.now_playing = None;

        let name = self.title(row_id, &path);
        let files = backend.files().clone();
        let fetch_path = path.clone();
        self.loading = Some(Loading {
            row_id,
            name,
            path,
            promise: TrackedPromise::spawn_local(async move {
                let scd = files.file::<SoundContainer>(&fetch_path).await?;
                let entry = scd
                    .entries()
                    .first()
                    .ok_or_else(|| anyhow!("no audio streams in {fetch_path}"))?;
                audio::decode(entry)
            }),
        });
    }

    fn start(&mut self, row_id: u32, name: String, path: String, decoded: Decoded) {
        if !self.ensure_player() {
            return;
        }
        let now_playing = NowPlaying {
            name: name.clone(),
            path,
            row_id,
            channels: decoded.channels,
            sample_rate: decoded.sample_rate,
            looping: decoded.loop_start.is_some(),
        };
        let player = self.player.as_mut().unwrap();
        player.set_volume(self.volume);
        if let Err(error) = player.play(decoded) {
            log::error!("BGM playback failed: {error}");
            return;
        }
        player.set_metadata(&name);
        self.now_playing = Some(now_playing);
    }

    fn ensure_player(&mut self) -> bool {
        if self.player.is_none() {
            match Player::new() {
                Ok(player) => self.player = Some(player),
                Err(error) => {
                    log::error!("audio init failed: {error}");
                    return false;
                }
            }
        }
        true
    }

    fn player_bar(&mut self, ui: &mut egui::Ui) {
        enum Cmd {
            Toggle,
            Stop,
            Scrub(f64),
            Seek(f64),
            Volume(f32),
        }
        let playing = self.player.as_ref().is_some_and(Player::is_playing);
        let has_track = self.now_playing.is_some();
        let (position, duration) = self
            .player
            .as_ref()
            .map_or((0.0, 0.0), |player| (player.position(), player.duration()));
        let looping = self.now_playing.as_ref().is_some_and(|now| now.looping);
        let mut volume = self.volume;
        let bar_position = self.scrub.unwrap_or(position);
        let mut command = None;

        ui.horizontal(|ui| {
            ui.add_enabled_ui(has_track, |ui| {
                if ui.button(if playing { "⏸" } else { "▶" }).clicked() {
                    command = Some(Cmd::Toggle);
                }
                if ui.button("⏹").clicked() {
                    command = Some(Cmd::Stop);
                }
            });

            ui.label(format_time(bar_position));
            let mut seek = bar_position;
            let response = ui.add_enabled(
                has_track && duration > 0.0,
                Slider::new(&mut seek, 0.0..=duration.max(0.001)).show_value(false),
            );
            if response.dragged() {
                command = Some(Cmd::Scrub(seek));
            } else if response.drag_stopped() || response.changed() {
                command = Some(Cmd::Seek(seek));
            }
            ui.label(format_time(duration));
            if looping {
                ui.label("🔁").on_hover_text("Loops");
            }

            ui.separator();
            ui.label("🔊");
            if ui
                .add(Slider::new(&mut volume, 0.0..=1.0).show_value(false))
                .changed()
            {
                command = Some(Cmd::Volume(volume));
            }
        });

        match command {
            Some(Cmd::Toggle) => {
                if let Some(player) = &self.player {
                    if playing {
                        player.pause();
                    } else {
                        player.resume();
                    }
                }
            }
            Some(Cmd::Stop) => {
                if let Some(player) = &mut self.player {
                    player.stop();
                }
                self.now_playing = None;
                self.scrub = None;
            }
            Some(Cmd::Scrub(seconds)) => self.scrub = Some(seconds),
            Some(Cmd::Seek(seconds)) => {
                if let Some(player) = &mut self.player {
                    player.seek(seconds);
                }
                self.scrub = None;
            }
            Some(Cmd::Volume(value)) => {
                self.volume = value;
                if let Some(player) = &mut self.player {
                    player.set_volume(value);
                }
            }
            None => {}
        }

        if let Some(now) = &self.now_playing {
            let locations = self
                .songs
                .get(&now.row_id)
                .filter(|song| !song.locations.is_empty())
                .map(|song| format!("  ·  {}", song.locations))
                .unwrap_or_default();
            ui.label(RichText::new(format!("{}{locations}", now.name)).strong());
            ui.label(
                RichText::new(format!(
                    "{}  ·  {} ch, {} Hz{}",
                    now.path,
                    now.channels,
                    now.sample_rate,
                    if now.looping { ", looping" } else { "" },
                ))
                .weak(),
            );
        } else if self.loading.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Decoding…");
            });
        }
    }

    fn track_list(&mut self, ui: &mut egui::Ui, backend: &Backend) {
        let unavailable_count = match (&self.index, &self.avail) {
            (Index::Loaded(tracks), Avail::Ready(available)) => {
                tracks.iter().filter(|t| !available.contains(&t.path)).count()
            }
            _ => 0,
        };

        ui.horizontal(|ui| {
            ui.label("Filter:");
            ui.text_edit_singleline(&mut self.search);
            if !self.search.is_empty() && ui.button("Clear").clicked() {
                self.search.clear();
            }
            if unavailable_count > 0 {
                ui.checkbox(
                    &mut self.show_unavailable,
                    format!("Show {unavailable_count} unavailable"),
                )
                .on_hover_text("Tracks not served by this data source (the web backend is ffxiv/ARR only)");
            }
        });

        let query = self.search.to_lowercase();
        let current = self.now_playing.as_ref().map(|now| now.row_id);
        let mut clicked = None;

        match &self.index {
            Index::Idle | Index::Loading(_) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Loading BGM list…");
                });
            }
            Index::Failed(error) => {
                ui.colored_label(Color32::RED, format!("Failed to load BGM list: {error}"));
            }
            Index::Loaded(tracks) if tracks.is_empty() => {
                ui.label("No BGM tracks found.");
            }
            Index::Loaded(tracks) => {
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for track in tracks {
                            let name = self.title(track.row_id, &track.path);
                            if !query.is_empty() && !name.to_lowercase().contains(&query) {
                                continue;
                            }
                            let available = match &self.avail {
                                Avail::Ready(set) => set.contains(&track.path),
                                _ => true,
                            };
                            if !available && !self.show_unavailable {
                                continue;
                            }
                            let selected = current == Some(track.row_id);
                            let response = ui
                                .add_enabled_ui(available, |ui| ui.selectable_label(selected, &name))
                                .inner
                                .on_hover_ui(|ui| self.track_hover(ui, track, &name, available));
                            if response.clicked() {
                                clicked = Some(track.row_id);
                            }
                        }
                    });
            }
        }

        if let Some(row_id) = clicked {
            let path = if let Index::Loaded(tracks) = &self.index {
                tracks
                    .iter()
                    .find(|t| t.row_id == row_id)
                    .map(|t| t.path.clone())
            } else {
                None
            };
            if let Some(path) = path {
                self.begin_load(backend, row_id, path);
            }
        }
    }

    fn track_hover(&self, ui: &mut egui::Ui, track: &BgmTrack, name: &str, available: bool) {
        ui.strong(name);
        if let Some(song) = self.songs.get(&track.row_id) {
            if !song.alt.is_empty() {
                ui.label(format!("Also known as: {}", song.alt));
            }
            if !song.special.is_empty() {
                ui.label(format!("Special mode: {}", song.special));
            }
            if !song.locations.is_empty() {
                ui.label(format!("Locations: {}", song.locations));
            }
            if !song.info.is_empty() {
                ui.label(format!("Notes: {}", song.info));
            }
            if song.duration > 0 {
                ui.label(format!("Duration: {}", format_time(f64::from(song.duration))));
            }
        }
        ui.separator();
        ui.label(RichText::new(&track.path).weak());
        ui.label(RichText::new(format!("BGM #{}", track.row_id)).weak());
        if !available {
            ui.colored_label(Color32::from_rgb(0xE0, 0x8C, 0x3C), "Not available on this data source");
        }
    }
}

fn format_time(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "0:00".to_string();
    }
    let total = seconds as u64;
    format!("{}:{:02}", total / 60, total % 60)
}

fn file_stem(path: &str) -> String {
    path.rsplit('/')
        .next()
        .unwrap_or(path)
        .trim_end_matches(".scd")
        .to_string()
}

async fn load_index(excel: CachedProvider) -> Result<Vec<BgmTrack>> {
    let sheet = excel.get_sheet("BGM", Language::None).await?;
    let offset = u32::from(
        sheet
            .columns()
            .first()
            .ok_or_else(|| anyhow!("BGM sheet has no columns"))?
            .offset(),
    );

    let mut tracks = Vec::new();
    for row_id in sheet.get_row_ids() {
        let Ok(row) = sheet.get_row(row_id) else {
            continue;
        };
        let Ok(cell) = row.read_string(offset) else {
            continue;
        };
        let path = String::from_utf8_lossy(cell.as_bytes()).into_owned();
        if path.ends_with(".scd") {
            tracks.push(BgmTrack { row_id, path });
        }
    }
    Ok(tracks)
}

async fn check_availability(files: Rc<dyn FileProvider>, paths: Vec<String>) -> Result<HashSet<String>> {
    let mut available = HashSet::with_capacity(paths.len());
    for chunk in paths.chunks(100) {
        let exists = files.exists_many(chunk).await?;
        for (path, ok) in chunk.iter().zip(exists) {
            if ok {
                available.insert(path.clone());
            }
        }
    }
    Ok(available)
}
