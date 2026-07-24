use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::rc::Rc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use egui::{
    Align, Button, CentralPanel, Color32, Label, Layout, Rect, RichText, ScrollArea, Sense, Slider,
    TextEdit, UiBuilder, Vec2, Widget, containers::panel::Panel, pos2, vec2,
};
use ironworks::excel::Language;
use ironworks::file::File;
use ironworks::file::scd::{Codec, SoundContainer};
use serde::Deserialize;

use crate::audio::{self, Decoded, Player};
use crate::backend::Backend;
use crate::data::{FileProvider, FileProviderExt};
use crate::excel::base::CachedProvider;
use crate::excel::provider::{ExcelHeader, ExcelProvider, ExcelSheet};
use crate::settings::{BACKEND_CONFIG, InstallLocation, LANGUAGE};
use crate::utils::{
    CollapsibleSidePanel, FuzzyMatcher, PromiseKind, Side, TrackedPromise, fetch_url_str,
};

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

enum Avail {
    Idle,
    Loading(TrackedPromise<Result<HashSet<String>>>),
    Ready(HashSet<String>),
    Failed,
}

enum Songs {
    Idle,
    Loading(TrackedPromise<Result<HashMap<u32, SongInfo>>>),
    Done,
}

#[derive(Clone, Copy)]
struct StreamInfo {
    codec: Codec,
    file_size: usize,
    stream_size: usize,
}

enum Stage {
    Downloading(TrackedPromise<Result<(SoundContainer, usize)>>),
    Decoding(StreamInfo, TrackedPromise<Result<Decoded>>),
}

struct Loading {
    row_id: u32,
    name: String,
    path: String,
    stage: Stage,
}

impl Loading {
    fn phase(&self) -> &'static str {
        match self.stage {
            Stage::Downloading(_) => "Downloading",
            Stage::Decoding(..) => "Decoding",
        }
    }
}

struct NowPlaying {
    name: String,
    path: String,
    row_id: u32,
    channels: u16,
    sample_rate: u32,
    loop_range_secs: Option<(f64, f64)>,
    info: StreamInfo,
}

struct TrackRow {
    row_id: u32,
    path: String,
    name: String,
    available: bool,
}

pub struct MusicPlayer {
    player: Option<Player>,
    index: Index,
    avail: Avail,
    songs: HashMap<u32, SongInfo>,
    songs_load: Songs,
    songs_lang: Option<Language>,
    loading: Option<Loading>,
    now_playing: Option<NowPlaying>,
    pending: Option<u32>,
    volume: f32,
    search: String,
    show_unavailable: bool,
    show_visualizer: bool,
    rows: Vec<TrackRow>,
    rows_stale: bool,
    matcher: FuzzyMatcher,
    scrub: Option<f64>,
    viz: Vec<f32>,
}

impl Default for MusicPlayer {
    fn default() -> Self {
        Self {
            player: None,
            index: Index::Idle,
            avail: Avail::Idle,
            songs: HashMap::new(),
            songs_load: Songs::Idle,
            songs_lang: None,
            loading: None,
            now_playing: None,
            pending: None,
            volume: 1.0,
            search: String::new(),
            show_unavailable: false,
            show_visualizer: true,
            rows: Vec::new(),
            rows_stale: true,
            matcher: FuzzyMatcher::new(),
            scrub: None,
            viz: Vec::new(),
        }
    }
}

enum Cmd {
    Toggle,
    Stop,
    Scrub(f64),
    Seek(f64),
    Volume(f32),
    ToggleVisualizer,
}

impl MusicPlayer {
    pub fn request(&mut self, row_id: u32) {
        self.pending = Some(row_id);
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, backend: &Backend) -> Option<u32> {
        let api_url = match BACKEND_CONFIG.get(ui.ctx()) {
            Some(config) => match config.location {
                InstallLocation::Web(url, ..) => Some(url),
                _ => None,
            },
            None => None,
        };
        self.poll(backend, api_url, LANGUAGE.get(ui.ctx()));

        let playing = self.player.as_ref().is_some_and(Player::is_playing);
        if playing && self.show_visualizer && self.now_playing.is_some() {
            ui.ctx().request_repaint();
        } else if playing || self.loading.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }

        let clicked = self.side_panel(ui);
        self.now_playing_panel(ui);
        clicked
    }

    fn poll(&mut self, backend: &Backend, api_url: Option<String>, lang: Language) {
        if matches!(self.index, Index::Idle) {
            let excel = backend.excel().clone();
            self.index = Index::Loading(TrackedPromise::spawn_local(async move {
                load_index(excel).await
            }));
        }
        if matches!(&self.index, Index::Loading(p) if p.try_get().is_some()) {
            let Index::Loading(promise) = std::mem::replace(&mut self.index, Index::Idle) else {
                unreachable!()
            };
            self.index = match promise.block_and_take() {
                Ok(tracks) => Index::Loaded(tracks),
                Err(error) => Index::Failed(error.to_string()),
            };
            self.rows_stale = true;
        }

        if let Some(url) = &api_url
            && self.songs_lang != Some(lang)
            && !matches!(self.songs_load, Songs::Loading(_))
        {
            self.songs_lang = Some(lang);
            let url = format!("{}/songs/{}/", url.trim_end_matches('/'), song_sheet(lang));
            self.songs_load = Songs::Loading(TrackedPromise::spawn_local(async move {
                Ok(serde_json::from_str(&fetch_url_str(url).await?)?)
            }));
        }
        if matches!(&self.songs_load, Songs::Loading(p) if p.try_get().is_some()) {
            let Songs::Loading(promise) = std::mem::replace(&mut self.songs_load, Songs::Idle)
            else {
                unreachable!()
            };
            match promise.block_and_take() {
                Ok(songs) => {
                    self.songs = songs;
                    self.rows_stale = true;
                }
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
            self.rows_stale = true;
        }

        self.poll_loading();

        if let Some(row_id) = self.pending {
            let active = self.now_playing.as_ref().map(|n| n.row_id) == Some(row_id)
                || self.loading.as_ref().map(|l| l.row_id) == Some(row_id);
            if active {
                self.pending = None;
            } else if let Index::Loaded(tracks) = &self.index {
                let path = tracks
                    .iter()
                    .find(|t| t.row_id == row_id)
                    .map(|t| t.path.clone());
                self.pending = None;
                if let Some(path) = path {
                    self.begin_load(backend, row_id, path);
                }
            }
        }
    }

    fn poll_loading(&mut self) {
        let ready = match &self.loading {
            Some(l) => match &l.stage {
                Stage::Downloading(p) => p.try_get().is_some(),
                Stage::Decoding(_, p) => p.try_get().is_some(),
            },
            None => return,
        };
        if !ready {
            return;
        }
        let Loading {
            row_id,
            name,
            path,
            stage,
        } = self.loading.take().unwrap();
        match stage {
            Stage::Downloading(promise) => match promise.block_and_take() {
                Ok((container, file_size)) => {
                    let Some(info) = stream_info(&container, file_size) else {
                        log::error!("no audio streams in {path}");
                        return;
                    };
                    let decode = TrackedPromise::spawn_local(async move {
                        let entry = container
                            .entries()
                            .first()
                            .ok_or_else(|| anyhow!("no audio streams"))?;
                        audio::decode(entry)
                    });
                    self.loading = Some(Loading {
                        row_id,
                        name,
                        path,
                        stage: Stage::Decoding(info, decode),
                    });
                }
                Err(error) => log::error!("BGM download failed: {error}"),
            },
            Stage::Decoding(info, promise) => match promise.block_and_take() {
                Ok(decoded) => self.start(row_id, name, path, info, decoded),
                Err(error) => log::error!("BGM decode failed: {error}"),
            },
        }
    }

    fn title(&self, row_id: u32, path: &str) -> String {
        self.songs
            .get(&row_id)
            .filter(|song| !song.title.is_empty())
            .map_or_else(|| file_stem(path), |song| song.title.clone())
    }

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
        let promise = TrackedPromise::spawn_local(async move {
            let bytes = files.file::<Vec<u8>>(&fetch_path).await?;
            let file_size = bytes.len();
            let container = SoundContainer::read(Cursor::new(bytes))?;
            Ok((container, file_size))
        });
        self.loading = Some(Loading {
            row_id,
            name,
            path,
            stage: Stage::Downloading(promise),
        });
    }

    fn start(
        &mut self,
        row_id: u32,
        name: String,
        path: String,
        info: StreamInfo,
        decoded: Decoded,
    ) {
        if !self.ensure_player() {
            return;
        }
        let rate = f64::from(decoded.sample_rate);
        let now_playing = NowPlaying {
            name: name.clone(),
            path,
            row_id,
            channels: decoded.channels,
            sample_rate: decoded.sample_rate,
            loop_range_secs: decoded
                .loop_start
                .zip(decoded.loop_end)
                .map(|(start, end)| (f64::from(start) / rate, f64::from(end) / rate)),
            info,
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

    fn rebuild_rows(&mut self) {
        let Index::Loaded(tracks) = &self.index else {
            return;
        };
        let rows = tracks
            .iter()
            .map(|track| {
                let name = self
                    .songs
                    .get(&track.row_id)
                    .filter(|song| !song.title.is_empty())
                    .map_or_else(|| file_stem(&track.path), |song| song.title.clone());
                let available = match &self.avail {
                    Avail::Ready(set) => set.contains(&track.path),
                    _ => true,
                };
                TrackRow {
                    row_id: track.row_id,
                    path: track.path.clone(),
                    name,
                    available,
                }
            })
            .collect();
        self.rows = rows;
        self.rows_stale = false;
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) -> Option<u32> {
        if self.rows_stale {
            self.rebuild_rows();
        }
        let mut clicked = None;
        CollapsibleSidePanel::new("music_list", Side::Left).show(ui, |ui, is_open| {
            if !is_open {
                return;
            }
            Panel::top("music_list_header").show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                        CollapsibleSidePanel::draw_arrow(ui, "music_list");
                        ui.vertical_centered_justified(|ui| ui.heading("Tracks"));
                    });
                });
                ui.add_space(4.0);
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    if ui
                        .add_enabled(!self.search.is_empty(), Button::new("↩"))
                        .on_hover_text("Clear")
                        .clicked()
                    {
                        self.search.clear();
                    }
                    let unavailable = self.rows.iter().filter(|row| !row.available).count();
                    if unavailable > 0 {
                        ui.toggle_value(&mut self.show_unavailable, "🚫")
                            .on_hover_text(format!("Show {unavailable} unavailable"));
                    }
                    ui.add_sized(
                        Vec2::new(ui.available_width(), 0.0),
                        TextEdit::singleline(&mut self.search).hint_text("Filter"),
                    );
                });
                ui.add_space(4.0);
            });

            CentralPanel::default().show(ui, |ui| match &self.index {
                Index::Idle | Index::Loading(_) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Loading BGM list…");
                    });
                }
                Index::Failed(error) => {
                    ui.colored_label(Color32::RED, format!("Failed to load BGM list: {error}"));
                }
                Index::Loaded(_) => clicked = self.draw_rows(ui),
            });
        });
        clicked
    }

    fn draw_rows(&self, ui: &mut egui::Ui) -> Option<u32> {
        let selected = self
            .now_playing
            .as_ref()
            .map(|n| n.row_id)
            .or_else(|| self.loading.as_ref().map(|l| l.row_id));
        let query = (!self.search.is_empty()).then_some(self.search.as_str());
        let filtered: Vec<&TrackRow> = self.matcher.match_list_indirect(
            query,
            self.rows
                .iter()
                .filter(|row| self.show_unavailable || row.available),
            |row| row.name.as_str(),
        );

        let mut clicked = None;
        let row_height = ui.text_style_height(&egui::TextStyle::Button);
        ScrollArea::vertical().auto_shrink(false).show_rows(
            ui,
            row_height,
            filtered.len(),
            |ui, range| {
                ui.with_layout(Layout::top_down_justified(Align::Min), |ui| {
                    for row in &filtered[range] {
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                        let response = ui
                            .add_enabled_ui(row.available, |ui| {
                                Button::selectable(selected == Some(row.row_id), row.name.as_str())
                                    .ui(ui)
                            })
                            .inner
                            .on_hover_ui(|ui| self.row_hover(ui, row));
                        if response.clicked() {
                            clicked = Some(row.row_id);
                        }
                    }
                });
            },
        );
        clicked
    }

    fn row_hover(&self, ui: &mut egui::Ui, row: &TrackRow) {
        ui.strong(&row.name);
        if let Some(song) = self.songs.get(&row.row_id) {
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
                ui.label(format!(
                    "Duration: {}",
                    format_time(f64::from(song.duration))
                ));
            }
        }
        ui.separator();
        ui.label(RichText::new(&row.path).weak());
        ui.label(RichText::new(format!("BGM #{}", row.row_id)).weak());
        if !row.available {
            ui.colored_label(
                Color32::from_rgb(0xE0, 0x8C, 0x3C),
                "Not available on this data source",
            );
        }
    }

    fn now_playing_panel(&mut self, ui: &mut egui::Ui) {
        CentralPanel::default().show(ui, |ui| {
            if CollapsibleSidePanel::is_collapsed(ui.ctx(), "music_list") {
                Panel::top("music_reexpand").show(ui, |ui| {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| CollapsibleSidePanel::draw_arrow(ui, "music_list"));
                    ui.add_space(4.0);
                });
            }
            if self.now_playing.is_some() {
                self.draw_player(ui);
            } else if let Some(loading) = &self.loading {
                let phase = loading.phase();
                let name = loading.name.clone();
                center(ui, |ui| {
                    ui.spinner();
                    ui.add_space(8.0);
                    ui.label(RichText::new(format!("{phase}…")).heading());
                    ui.label(RichText::new(name).weak());
                });
            } else {
                center(ui, |ui| {
                    ui.label(RichText::new("♪").size(56.0).weak());
                    ui.label(RichText::new("Select a track to play").weak());
                });
            }
        });
    }

    fn draw_player(&mut self, ui: &mut egui::Ui) {
        let now = self.now_playing.as_ref().unwrap();
        let (name, path, loop_range, channels, sample_rate, info, row_id) = (
            now.name.clone(),
            now.path.clone(),
            now.loop_range_secs,
            now.channels,
            now.sample_rate,
            now.info,
            now.row_id,
        );
        let locations = self
            .songs
            .get(&row_id)
            .filter(|song| !song.locations.is_empty())
            .map(|song| song.locations.clone());
        let playing = self.player.as_ref().is_some_and(Player::is_playing);
        let (position, duration) = self
            .player
            .as_ref()
            .map_or((0.0, 0.0), |player| (player.position(), player.duration()));
        let bar_position = self.scrub.unwrap_or(position);
        let mut volume = self.volume;
        let show_viz = self.show_visualizer;

        let mut spectrum = [0u8; 4096];
        if show_viz && let Some(player) = &self.player {
            player.spectrum(&mut spectrum);
        }
        let bars = if show_viz {
            self.viz_bars(&spectrum, sample_rate, playing)
        } else {
            Vec::new()
        };

        let outer = ui.available_rect_before_wrap();
        let col_w = outer.width().min(600.0);
        let col_rect = Rect::from_min_size(
            pos2(outer.center().x - col_w / 2.0, outer.top()),
            vec2(col_w, outer.height()),
        );
        let mut cmd = None;
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(col_rect)
                .layout(Layout::top_down(Align::Center)),
            |ui| {
                let sp = ui.spacing().item_spacing.x;
                let h = ui.spacing().interact_size.y;

                ui.add_space(16.0);
                if show_viz {
                    draw_visualizer(ui, &bars, 176.0);
                    ui.add_space(18.0);
                }

                ui.label(RichText::new(&name).size(26.0).strong());
                if let Some(locations) = &locations {
                    ui.label(RichText::new(locations).weak());
                }
                ui.add_space(18.0);

                let time_w = 44.0;
                ui.horizontal(|ui| {
                    ui.add_sized(
                        vec2(time_w, h),
                        Label::new(RichText::new(format_time(bar_position)).weak()),
                    );
                    ui.spacing_mut().slider_width = (col_w - 2.0 * (time_w + sp)).max(80.0);
                    let mut seek = bar_position;
                    let response = ui.add_enabled(
                        duration > 0.0,
                        Slider::new(&mut seek, 0.0..=duration.max(0.001)).show_value(false),
                    );
                    if response.dragged() {
                        cmd = Some(Cmd::Scrub(seek));
                    } else if response.drag_stopped() || response.changed() {
                        cmd = Some(Cmd::Seek(seek));
                    }
                    ui.add_sized(
                        vec2(time_w, h),
                        Label::new(RichText::new(format_time(duration)).weak()),
                    );
                });
                ui.add_space(14.0);

                ui.horizontal(|ui| {
                    if ui
                        .add_sized(
                            vec2(40.0, 34.0),
                            Button::new(RichText::new(if playing { "⏸" } else { "▶" }).size(19.0)),
                        )
                        .clicked()
                    {
                        cmd = Some(Cmd::Toggle);
                    }
                    if ui
                        .add_sized(vec2(38.0, 34.0), Button::new(RichText::new("⏹").size(17.0)))
                        .clicked()
                    {
                        cmd = Some(Cmd::Stop);
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().slider_width = 150.0;
                        if ui
                            .add(Slider::new(&mut volume, 0.0..=1.0).show_value(false))
                            .changed()
                        {
                            cmd = Some(Cmd::Volume(volume));
                        }
                        ui.label("🔊");
                        if ui
                            .add_sized(vec2(40.0, 34.0), Button::selectable(show_viz, "📊"))
                            .on_hover_text("Visualizer")
                            .clicked()
                        {
                            cmd = Some(Cmd::ToggleVisualizer);
                        }
                    });
                });
                ui.add_space(18.0);

                draw_info(
                    ui,
                    &info,
                    channels,
                    sample_rate,
                    duration,
                    loop_range,
                    &path,
                );
            },
        );

        match cmd {
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
            Some(Cmd::ToggleVisualizer) => self.show_visualizer = !self.show_visualizer,
            None => {}
        }
    }

    fn viz_bars(&mut self, spectrum: &[u8], sample_rate: u32, playing: bool) -> Vec<f32> {
        if self.viz.len() != VIZ_BARS {
            self.viz = vec![0.0; VIZ_BARS];
        }
        if !playing {
            return self.viz.clone();
        }
        let bins = spectrum.len().max(1);
        let nyquist = (sample_rate as f32 / 2.0).max(1.0);
        let f_min = 40.0;
        let f_max = nyquist.min(16_000.0);
        let ratio = f_max / f_min;
        let bin_of = |freq: f32| ((freq / nyquist) * bins as f32).round() as usize;
        for (i, bar) in self.viz.iter_mut().enumerate() {
            let lo = bin_of(f_min * ratio.powf(i as f32 / VIZ_BARS as f32)).min(bins - 1);
            let hi =
                bin_of(f_min * ratio.powf((i + 1) as f32 / VIZ_BARS as f32)).clamp(lo + 1, bins);
            let peak = spectrum[lo..hi].iter().copied().max().unwrap_or(0) as f32 / 255.0;
            let target = peak.powf(1.4);
            let rate = if target > *bar { 0.55 } else { 0.16 };
            *bar += (target - *bar) * rate;
        }
        self.viz.clone()
    }
}

const VIZ_BARS: usize = 64;

fn draw_visualizer(ui: &mut egui::Ui, bars: &[f32], height: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 10.0, ui.visuals().extreme_bg_color);

    let inner = rect.shrink(12.0);
    let n = bars.len().max(1);
    let gap = 3.0;
    let bar_w = ((inner.width() - gap * (n as f32 - 1.0)) / n as f32).max(1.0);
    let accent = ui.visuals().selection.bg_fill;
    for (i, &m) in bars.iter().enumerate() {
        let m = m.clamp(0.0, 1.0);
        let bar_h = (m * inner.height()).max(2.0);
        let x = inner.left() + i as f32 * (bar_w + gap);
        let bar = Rect::from_min_max(
            pos2(x, inner.bottom() - bar_h),
            pos2(x + bar_w, inner.bottom()),
        );
        painter.rect_filled(bar, 2.0, accent.gamma_multiply(0.45 + 0.55 * m));
    }
}

fn draw_info(
    ui: &mut egui::Ui,
    info: &StreamInfo,
    channels: u16,
    sample_rate: u32,
    duration: f64,
    loop_range: Option<(f64, f64)>,
    path: &str,
) {
    let looping = loop_range.is_some();
    let bitrate = if duration > 0.0 {
        (info.stream_size as f64 * 8.0 / duration / 1000.0).round() as u64
    } else {
        0
    };
    let freq = if sample_rate.is_multiple_of(1000) {
        format!("{} kHz", sample_rate / 1000)
    } else {
        format!("{:.1} kHz", f64::from(sample_rate) / 1000.0)
    };
    let chan = match channels {
        1 => "Mono".to_string(),
        2 => "Stereo".to_string(),
        6 => "5.1".to_string(),
        n => format!("{n} ch"),
    };
    let sep = "   ·   ";
    let line1 = [codec_name(info.codec).to_string(), freq, chan].join(sep);
    let mut parts = vec![
        format!("{bitrate} kbps"),
        format_size(info.file_size),
        format_time(duration),
    ];
    if looping {
        parts.push("Looping".to_string());
    }
    let line2 = parts.join(sep);

    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
    ui.label(RichText::new(line1).weak());
    let stats = ui.label(RichText::new(line2).weak());
    if let Some((start, end)) = loop_range {
        stats.on_hover_text(format!(
            "Loops {} → {}",
            format_time(start),
            format_time(end)
        ));
    }
    ui.add_space(4.0);
    ui.label(RichText::new(path).weak().small());
}

fn center<R>(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.35);
        contents(ui)
    })
    .inner
}

fn codec_name(codec: Codec) -> &'static str {
    match codec {
        Codec::OggVorbis => "Ogg Vorbis",
        Codec::Hca => "HCA",
        Codec::Mp3 => "MP3",
        Codec::MsAdpcm => "MS ADPCM",
        Codec::Atrac9 => "ATRAC9",
        Codec::Pcm => "PCM",
        Codec::Empty => "Empty",
        Codec::Unknown(_) => "Unknown",
    }
}

fn format_size(bytes: usize) -> String {
    let bytes = bytes as f64;
    if bytes >= 1_048_576.0 {
        format!("{:.1} MB", bytes / 1_048_576.0)
    } else if bytes >= 1024.0 {
        format!("{:.0} KB", bytes / 1024.0)
    } else {
        format!("{bytes:.0} B")
    }
}

fn stream_info(container: &SoundContainer, file_size: usize) -> Option<StreamInfo> {
    let entry = container.entries().first()?;
    Some(StreamInfo {
        codec: entry.format(),
        file_size,
        stream_size: entry.data().len(),
    })
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

fn song_sheet(lang: Language) -> &'static str {
    match lang {
        Language::Japanese => "ja",
        Language::French => "fr",
        Language::German => "de",
        Language::ChineseSimplified | Language::ChineseTraditional | Language::TaiwanChinese => {
            "zh"
        }
        _ => "en",
    }
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

async fn check_availability(
    files: Rc<dyn FileProvider>,
    paths: Vec<String>,
) -> Result<HashSet<String>> {
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
