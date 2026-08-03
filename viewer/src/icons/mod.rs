mod refs;

use std::{cell::Cell, collections::HashSet, rc::Rc};

use anyhow::Result;
use egui::{
    Align, Button, CentralPanel, Color32, Layout, RichText, ScrollArea, Sense, TextEdit, Vec2,
    Widget, containers::panel::Panel,
};
use ironworks::excel::Language;
use itertools::Itertools;
use pathlist::{PathList, Presence};

use crate::{
    backend::Backend,
    data::{IconIndex, get_icon_path},
    excel::provider::ExcelProvider,
    goto::{ListNav, Palette, SUGGESTIONS},
    settings::{ALWAYS_HIRES, LANGUAGE, api_base},
    utils::{
        CollapsibleSidePanel, IconManager, ManagedIcon, PromiseKind, Side, TrackedPromise,
        icon_modal, yield_to_ui,
    },
};

use refs::{IconRefs, Progress, Use};

const ZOOM_STEPS: [f32; 6] = [32.0, 40.0, 48.0, 64.0, 80.0, 96.0];
/// How many cells the grid adds each time the scroll reaches the end.
const PAGE: usize = 360;
/// How many decoded icons the grid will let egui hold before it starts giving them back.
const LOADED_BUDGET: usize = 2048;

pub enum Action {
    /// An icon was picked; reflect it in the URL.
    Select(u32),
    /// A row naming the icon was clicked; hand off to the sheet tab.
    Navigate(String),
}

/// Which subset of the install's icons the grid is showing.
#[derive(Clone, PartialEq, Eq)]
enum Category {
    All,
    Localized,
    Unreferenced,
    Sheet(u16),
}

enum Load<T: Send + 'static> {
    Idle,
    Loading(TrackedPromise<Result<T>>),
    Ready(T),
    Failed(String),
}

pub struct IconBrowser {
    index: Load<()>,
    refs: Load<IconRefs>,
    progress: Rc<Cell<Progress>>,
    /// Every icon the install ships, ascending.
    all: Vec<u32>,
    localized: usize,
    /// Icons egui has decoded for the grid, in the order they were first drawn.
    loaded: Vec<(u32, String)>,
    loaded_ids: HashSet<u32>,
    /// `all` cut down to the picked category and the id filter; what the grid indexes.
    shown: Vec<u32>,
    shown_for: Option<(Category, String)>,
    category: Category,
    search: String,
    lookup: String,
    selected: Option<u32>,
    pending: Option<u32>,
    /// An icon the grid has yet to bring into view.
    scroll_to: Option<u32>,
    modal_icon: Option<u32>,
    pages: usize,
    zoom: usize,
    order_by_count: bool,
    palette: Option<Palette>,
    /// Keyboard cursor over the backreference list in the detail panel.
    nav: ListNav,
}

impl Default for IconBrowser {
    fn default() -> Self {
        Self {
            index: Load::Idle,
            refs: Load::Idle,
            progress: Rc::new(Cell::new(Progress::default())),
            all: Vec::new(),
            localized: 0,
            loaded: Vec::new(),
            loaded_ids: HashSet::new(),
            shown: Vec::new(),
            shown_for: None,
            category: Category::All,
            search: String::new(),
            lookup: String::new(),
            selected: None,
            pending: None,
            scroll_to: None,
            modal_icon: None,
            pages: 1,
            zoom: 1,
            order_by_count: true,
            palette: None,
            nav: ListNav::default(),
        }
    }
}

impl IconBrowser {
    pub fn selected(&self) -> Option<u32> {
        self.selected.or(self.pending)
    }

    /// Select the icon a deep link names, once there is an index to place it in.
    pub fn request(&mut self, icon_id: u32) {
        if self.selected != Some(icon_id) {
            self.pending = Some(icon_id);
        }
    }

    /// Drop everything that came from the install, so a reconnect reads it all again.
    pub fn reset(&mut self) {
        self.index = Load::Idle;
        self.refs = Load::Idle;
        self.all.clear();
        self.localized = 0;
        self.loaded.clear();
        self.loaded_ids.clear();
        self.shown.clear();
        self.shown_for = None;
        // Sheet categories are indices into the reverse index that just went away.
        self.category = Category::All;
        self.pending = self.pending.take().or(self.selected.take());
    }

    pub fn open_palette(&mut self) {
        self.palette = Some(Palette::new("Find Icon…", "Id", self.lookup.clone()));
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        backend: &Backend,
        icons: &IconManager,
    ) -> Option<Action> {
        self.poll(ui.ctx(), backend);
        if let Some(pending) = self.pending.take() {
            self.selected = Some(pending);
            self.scroll_to = Some(pending);
        }

        let picked = self.draw_palette(ui.ctx(), backend);
        let backreferences = self.selected.is_some()
            && matches!(self.refs, Load::Ready(_))
            && !CollapsibleSidePanel::is_collapsed(ui.ctx(), "icon_info");
        self.nav.claim(ui.ctx(), backreferences, None);

        self.side_panel(ui, backend);
        let followed = self.detail_panel(ui, backend, icons);
        let opened = self.grid_panel(ui, backend, icons);

        if let Some(icon_id) = self.modal_icon {
            let path = get_icon_path(backend.icons(), icon_id, true, LANGUAGE.get(ui.ctx()));
            let source = icon_source(icons, backend, ui.ctx(), &path);
            if icon_modal(ui.ctx(), icon_id, source) {
                self.modal_icon = None;
            }
        }

        picked
            .map(Action::Select)
            .or_else(|| followed.map(Action::Navigate))
            .or_else(|| opened.map(Action::Select))
    }

    fn draw_palette(&mut self, ctx: &egui::Context, backend: &Backend) -> Option<u32> {
        let palette = self.palette.take()?;
        match palette.draw(ctx, |query| {
            self.lookup = query.to_owned();
            self.rebuild_shown(backend);
            self.shown
                .iter()
                .take(SUGGESTIONS)
                .map(|id| (*id, format!("{id:06}")))
                .collect()
        }) {
            Ok(picked) => picked,
            Err(palette) => {
                self.palette = Some(palette);
                None
            }
        }
    }

    fn poll(&mut self, ctx: &egui::Context, backend: &Backend) {
        // The assets tab cuts the same index from the same fetch, but only once it has been opened.
        // Without this the tab shows nothing until the user has been there.
        if matches!(self.index, Load::Idle) {
            if backend.icons().is_some() {
                self.index = Load::Ready(());
            } else {
                let files = backend.files().clone();
                let api = api_base(ctx);
                let backend = backend.clone();
                self.index = Load::Loading(TrackedPromise::spawn_local(async move {
                    let (paths, presence) = files.path_index(&api).await?;
                    yield_to_ui().await;
                    let paths = PathList::decode(&paths)?;
                    let presence = Presence::decode(&presence)?;
                    backend.set_icons(IconIndex::build(&paths, &presence));
                    Ok(())
                }));
            }
        }
        if let Load::Loading(promise) = &self.index
            && let Some(result) = promise.try_get()
        {
            self.index = match result.as_ref().map_err(|e| e.to_string()) {
                Ok(()) => Load::Ready(()),
                Err(e) => Load::Failed(e),
            };
        }

        if self.all.is_empty()
            && matches!(self.index, Load::Ready(()))
            && let Some(icons) = backend.icons()
        {
            self.all = icons.ids().collect();
            self.localized = self.all.iter().filter(|id| icons.localized(**id)).count();
            self.shown_for = None;
        }

        if matches!(&self.refs, Load::Loading(p) if p.try_get().is_some()) {
            let Load::Loading(promise) = std::mem::replace(&mut self.refs, Load::Idle) else {
                unreachable!()
            };
            self.refs = match promise.block_and_take() {
                Ok(refs) => Load::Ready(refs),
                Err(error) => Load::Failed(error.to_string()),
            };
            self.shown_for = None;
        }
        if matches!(self.refs, Load::Loading(_)) {
            ctx.request_repaint();
        }
    }

    fn start_walk(&mut self, backend: &Backend) {
        let backend = backend.clone();
        let progress = self.progress.clone();
        self.refs = Load::Loading(TrackedPromise::spawn_local(async move {
            refs::walk(backend, progress).await
        }));
    }

    fn refs(&self) -> Option<&IconRefs> {
        match &self.refs {
            Load::Ready(refs) => Some(refs),
            _ => None,
        }
    }

    fn rebuild_shown(&mut self, backend: &Backend) {
        let key = (self.category.clone(), self.lookup.clone());
        if self.shown_for.as_ref() == Some(&key) {
            return;
        }
        self.shown_for = Some(key);
        self.pages = 1;

        self.shown = match (&self.category, self.refs()) {
            (Category::Localized, _) => match backend.icons() {
                Some(icons) => self
                    .all
                    .iter()
                    .copied()
                    .filter(|id| icons.localized(*id))
                    .collect(),
                None => Vec::new(),
            },
            (Category::Unreferenced, Some(refs)) => self
                .all
                .iter()
                .copied()
                .filter(|id| !refs.is_referenced(*id))
                .collect(),
            (Category::Sheet(sheet), Some(refs)) => refs.icons_of(*sheet),
            _ => self.all.clone(),
        };

        let lookup = self.lookup.trim();
        if !lookup.is_empty() {
            self.shown.retain(|id| format!("{id:06}").contains(lookup));
            // The list only names paths that have been observed, so it misses icons an install
            // ships. An id given in full is offered whether or not the list knows it.
            if let Ok(icon_id) = lookup.parse::<u32>()
                && let Err(at) = self.shown.binary_search(&icon_id)
            {
                self.shown.insert(at, icon_id);
            }
        }
    }

    fn side_panel(&mut self, ui: &mut egui::Ui, backend: &Backend) {
        CollapsibleSidePanel::new("icon_tree", Side::Left).show(ui, |ui, is_open| {
            if !is_open {
                return;
            }
            Panel::top("icon_tree_header").show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                        CollapsibleSidePanel::draw_arrow(ui, "icon_tree", Side::Left);
                        ui.vertical_centered_justified(|ui| ui.heading("Icons"));
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
                    ui.toggle_value(&mut self.order_by_count, "🔢")
                        .on_hover_text("Order by count");
                    ui.add_sized(
                        Vec2::new(ui.available_width(), 0.0),
                        TextEdit::singleline(&mut self.search).hint_text("Search categories"),
                    );
                });
                ui.add_space(4.0);
            });

            CentralPanel::default().show(ui, |ui| self.draw_categories(ui, backend));
        });
    }

    fn draw_categories(&mut self, ui: &mut egui::Ui, backend: &Backend) {
        let localized = self.localized;
        let query = self.search.to_lowercase();

        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.with_layout(Layout::top_down_justified(Align::Min), |ui| {
                let mut category = self.category.clone();
                let mut select = |ui: &mut egui::Ui, what: Category, label: String| {
                    if Button::selectable(category == what, label).ui(ui).clicked() {
                        category = what;
                    }
                };

                if query.is_empty() {
                    select(
                        ui,
                        Category::All,
                        format!("All icons ({})", thousands(self.all.len())),
                    );
                }

                match &self.refs {
                    Load::Ready(refs) => {
                        ui.add_space(4.0);
                        let mut sheets: Vec<(u16, &str, u32)> = refs
                            .sheets()
                            .filter(|(_, name, count)| {
                                *count > 0
                                    && (query.is_empty() || name.to_lowercase().contains(&query))
                            })
                            .collect();
                        if self.order_by_count {
                            sheets
                                .sort_by_key(|(_, name, count)| (std::cmp::Reverse(*count), *name));
                        } else {
                            sheets.sort_by_key(|(_, name, _)| *name);
                        }
                        for (sheet, name, count) in sheets {
                            select(
                                ui,
                                Category::Sheet(sheet),
                                format!("{name} ({})", thousands(count as usize)),
                            );
                        }

                        if query.is_empty() {
                            ui.add_space(8.0);
                            ui.label(RichText::new("OTHER SETS").weak().small());
                            select(
                                ui,
                                Category::Localized,
                                format!("Language icons ({})", thousands(localized)),
                            );
                            select(
                                ui,
                                Category::Unreferenced,
                                format!(
                                    "Other icons ({})",
                                    thousands(self.all.len().saturating_sub(refs.referenced()))
                                ),
                            );
                        }
                    }
                    Load::Loading(_) => {
                        let progress = self.progress.get();
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(format!(
                                "{} {}/{}",
                                if progress.reading_rows {
                                    "Reading sheets…"
                                } else {
                                    "Reading schemas…"
                                },
                                thousands(progress.done),
                                thousands(progress.total)
                            ));
                        });
                    }
                    Load::Failed(error) => {
                        ui.add_space(8.0);
                        ui.colored_label(Color32::RED, error.clone());
                    }
                    Load::Idle => {
                        ui.add_space(8.0);
                        if query.is_empty()
                            && ui
                                .button("Load Backreferences")
                                .on_hover_text(
                                    "Reads every sheet that names an icon so each one can list \
                                     the rows using it. Tens of megabytes.",
                                )
                                .clicked()
                        {
                            self.start_walk(backend);
                        }
                        if query.is_empty() {
                            select(
                                ui,
                                Category::Localized,
                                format!("Language icons ({})", thousands(localized)),
                            );
                        }
                    }
                }
                self.category = category;
            });
        });
    }

    fn grid_panel(
        &mut self,
        ui: &mut egui::Ui,
        backend: &Backend,
        icons: &IconManager,
    ) -> Option<u32> {
        self.rebuild_shown(backend);
        let mut opened = None;
        CentralPanel::default().show(ui, |ui| {
            Panel::top("icon_grid_header").show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if CollapsibleSidePanel::is_collapsed(ui.ctx(), "icon_tree") {
                        CollapsibleSidePanel::draw_arrow(ui, "icon_tree", Side::Left);
                    }
                    let capped = self.shown.len().min(self.pages * PAGE);
                    ui.label(if capped < self.shown.len() {
                        format!("{} icons, scroll for more", thousands(capped))
                    } else {
                        format!("{} icons", thousands(self.shown.len()))
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if CollapsibleSidePanel::is_collapsed(ui.ctx(), "icon_info") {
                            CollapsibleSidePanel::draw_arrow(ui, "icon_info", Side::Right);
                        }
                        if ui
                            .add_enabled(self.zoom + 1 < ZOOM_STEPS.len(), Button::new("+"))
                            .on_hover_text("Zoom in")
                            .clicked()
                        {
                            self.zoom += 1;
                        }
                        if ui
                            .add_enabled(self.zoom > 0, Button::new("−"))
                            .on_hover_text("Zoom out")
                            .clicked()
                        {
                            self.zoom -= 1;
                        }
                        if capped < self.shown.len() && ui.button("Load all").clicked() {
                            self.pages = self.shown.len().div_ceil(PAGE);
                        }
                        ui.add_sized(
                            Vec2::new(90.0, ui.spacing().interact_size.y),
                            TextEdit::singleline(&mut self.lookup).hint_text("Id"),
                        );
                    });
                });
                ui.add_space(4.0);
            });

            CentralPanel::default().show(ui, |ui| match &self.index {
                Load::Idle | Load::Loading(_) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Loading path list…");
                    });
                }
                Load::Failed(error) => {
                    ui.colored_label(Color32::RED, error.clone());
                }
                Load::Ready(()) => opened = self.draw_grid(ui, backend, icons),
            });
        });
        opened
    }

    fn draw_grid(
        &mut self,
        ui: &mut egui::Ui,
        backend: &Backend,
        icons: &IconManager,
    ) -> Option<u32> {
        let cell = ZOOM_STEPS[self.zoom];
        let label_height = ui.text_style_height(&egui::TextStyle::Small);
        let spacing = ui.spacing().item_spacing;
        let step = cell + spacing.y + label_height;
        let columns = (((ui.available_width() + spacing.x) / (cell + spacing.x)) as usize).max(1);

        let scroll_row = self.scroll_to.take().and_then(|icon_id| {
            let slot = self.shown.binary_search(&icon_id).ok()?;
            self.pages = self.pages.max((slot + 1).div_ceil(PAGE));
            Some(slot / columns)
        });

        let capped = self.shown.len().min(self.pages * PAGE);
        let rows = capped.div_ceil(columns);
        let hires = ALWAYS_HIRES.get(ui.ctx());
        let language = LANGUAGE.get(ui.ctx());

        let mut area = ScrollArea::vertical().auto_shrink(false);
        if let Some(row) = scroll_row {
            // A row off the drawn window has no widget to scroll to, so place the offset by hand.
            // `show_rows` pitches rows by the height it is given plus one item spacing.
            let pitch = step + spacing.y;
            let height = ui.available_height();
            let last = (rows as f32 * pitch - spacing.y - height).max(0.0);
            area = area.vertical_scroll_offset(
                (row as f32 * pitch + (step - height) / 2.0).clamp(0.0, last),
            );
        }

        let mut opened = None;
        let mut at_end = false;
        let mut on_screen = 0..0;
        let mut drawn = Vec::new();
        area.show_rows(ui, step, rows, |ui, range| {
            at_end = range.end >= rows;
            on_screen = (range.start * columns)..(range.end * columns).min(capped);
            for row in range {
                ui.horizontal(|ui| {
                    for slot in row * columns..((row + 1) * columns).min(capped) {
                        let icon_id = self.shown[slot];
                        let (clicked, uri) =
                            self.draw_cell(ui, backend, icons, icon_id, cell, hires, language);
                        if clicked {
                            opened = Some(icon_id);
                        }
                        if let Some(uri) = uri {
                            drawn.push((icon_id, uri));
                        }
                    }
                });
            }
        });

        // A click settles the selection here so that the trip back through the URL does not read as
        // a fresh request and scroll the grid out from under it.
        if let Some(icon_id) = opened {
            self.selected = Some(icon_id);
        }

        for (icon_id, uri) in drawn {
            if self.loaded_ids.insert(icon_id) {
                self.loaded.push((icon_id, uri));
            }
        }
        // `shown` is ascending, so what is on screen is every id between its ends.
        let visible = &self.shown[on_screen];
        let visible = visible
            .first()
            .zip(visible.last())
            .map(|(low, high)| *low..=*high);
        self.evict(ui.ctx(), visible);

        // The cap grows only once the last row is on screen, which raises the scroll range by a
        // page and so takes the position off the end again.
        if at_end && capped < self.shown.len() {
            self.pages += 1;
            ui.ctx().request_repaint();
        }
        opened
    }

    /// Hand back the pixels of icons that have scrolled away. egui owns the decoded texture behind
    /// a `Uri` source, and nothing else ever drops it.
    fn evict(&mut self, ctx: &egui::Context, on_screen: Option<std::ops::RangeInclusive<u32>>) {
        if self.loaded.len() <= LOADED_BUDGET {
            return;
        }
        let mut over = self.loaded.len() - LOADED_BUDGET;
        let loaded_ids = &mut self.loaded_ids;
        self.loaded.retain(|(icon_id, uri)| {
            if over == 0
                || on_screen
                    .as_ref()
                    .is_some_and(|range| range.contains(icon_id))
            {
                return true;
            }
            ctx.forget_image(uri);
            loaded_ids.remove(icon_id);
            over -= 1;
            false
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_cell(
        &self,
        ui: &mut egui::Ui,
        backend: &Backend,
        icons: &IconManager,
        icon_id: u32,
        cell: f32,
        hires: bool,
        language: Language,
    ) -> (bool, Option<String>) {
        let path = get_icon_path(backend.icons(), icon_id, hires, language);
        let source = icon_source(icons, backend, ui.ctx(), &path);

        let uri = match &source {
            ManagedIcon::Loaded(egui::ImageSource::Uri(uri))
                if !self.loaded_ids.contains(&icon_id) =>
            {
                Some(uri.to_string())
            }
            _ => None,
        };
        let cell_ui = ui.vertical(|ui| {
            ui.set_width(cell);
            let response = match source {
                ManagedIcon::Loaded(image) => {
                    // Every cell takes the same square whatever shape the icon turns out to be, so
                    // that the ids under them stay on one line.
                    let (rect, response) =
                        ui.allocate_exact_size(Vec2::splat(cell), Sense::click());
                    fit_into(ui, image, rect);
                    response
                }
                ManagedIcon::Failed(_) => {
                    let (rect, response) =
                        ui.allocate_exact_size(Vec2::splat(cell), Sense::click());
                    ui.painter().rect_stroke(
                        rect,
                        2.0,
                        (1.0, ui.visuals().weak_text_color().gamma_multiply(0.4)),
                        egui::StrokeKind::Inside,
                    );
                    response
                }
                ManagedIcon::Loading | ManagedIcon::NotLoaded => {
                    let (rect, response) =
                        ui.allocate_exact_size(Vec2::splat(cell), Sense::click());
                    egui::Spinner::new().paint_at(ui, rect);
                    response
                }
            };
            let clicked = response
                .on_hover_text(format!("Id: {icon_id}\nPath: {path}"))
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked();
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(format!("{icon_id:06}")).small());
            });
            clicked
        });
        if self.selected == Some(icon_id) {
            // Rows sit one item spacing apart, so the stroke only has room to grow by half of it.
            ui.painter().rect_stroke(
                cell_ui.response.rect.expand2(Vec2::new(2.0, 1.0)),
                2.0,
                ui.visuals().selection.stroke,
                egui::StrokeKind::Outside,
            );
        }
        (cell_ui.inner, uri)
    }

    fn detail_panel(
        &mut self,
        ui: &mut egui::Ui,
        backend: &Backend,
        icons: &IconManager,
    ) -> Option<String> {
        let mut followed = None;
        let mut nav = std::mem::take(&mut self.nav);
        CollapsibleSidePanel::new("icon_info", Side::Right)
            .collapsed_width(0.0)
            .show(ui, |ui, is_open| {
                if !is_open {
                    return;
                }
                let Some(icon_id) = self.selected else {
                    ui.centered_and_justified(|ui| {
                        ui.label(RichText::new("No icon selected").weak());
                    });
                    return;
                };

                Panel::top("icon_info_header").show(ui, |ui| {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        CollapsibleSidePanel::draw_arrow(ui, "icon_info", Side::Right);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            // Balances the arrow, so the heading centers on the panel rather than
                            // on the space left over beside it.
                            ui.add_space(ui.spacing().indent);
                            ui.vertical_centered_justified(|ui| {
                                ui.heading(format!("Icon {icon_id:06}"));
                            });
                        });
                    });
                    ui.add_space(4.0);
                });

                CentralPanel::default().show(ui, |ui| {
                    followed = self.draw_detail(ui, backend, icons, icon_id, &mut nav);
                });
            });
        self.nav = nav;
        followed
    }

    fn draw_detail(
        &mut self,
        ui: &mut egui::Ui,
        backend: &Backend,
        icons: &IconManager,
        icon_id: u32,
        nav: &mut ListNav,
    ) -> Option<String> {
        let hires = ALWAYS_HIRES.get(ui.ctx());
        let language = LANGUAGE.get(ui.ctx());
        let path = get_icon_path(backend.icons(), icon_id, hires, language);

        let source = icon_source(icons, backend, ui.ctx(), &path);
        let bounds = Vec2::splat(ui.available_width().min(192.0));
        let mut size = None;
        let zoomed = ui
            .vertical_centered(|ui| match source {
                ManagedIcon::Loaded(image) => {
                    size = pixel_size(ui.ctx(), &image);
                    let image = egui::Image::new(image).maintain_aspect_ratio(true);
                    let fitted = image.load_and_calc_size(ui, bounds).unwrap_or(bounds);
                    let (rect, response) = ui.allocate_exact_size(fitted, Sense::click());
                    checkerboard(ui, rect);
                    image.paint_at(ui, rect);
                    response
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                }
                ManagedIcon::Failed(_) => {
                    ui.colored_label(Color32::RED, "Failed to load icon");
                    false
                }
                ManagedIcon::Loading | ManagedIcon::NotLoaded => {
                    ui.spinner();
                    false
                }
            })
            .inner;
        if zoomed {
            self.modal_icon = Some(icon_id);
        }

        ui.add_space(4.0);
        ui.label(
            RichText::new(match size {
                Some([w, h]) => format!("{w} × {h} · {path}"),
                None => path.clone(),
            })
            .weak()
            .small(),
        );
        if backend
            .icons()
            .is_some_and(|index| index.localized(icon_id))
        {
            ui.label(RichText::new("Has a language-specific file").weak().small());
        }
        if backend.icons().is_some_and(|index| !index.hires(icon_id)) {
            ui.label(RichText::new("No _hr1 file").weak().small());
        }

        ui.add_space(8.0);
        let mut followed = None;
        match self.refs() {
            None => {
                ui.label(RichText::new("Load Backreferences to see usages").weak());
            }
            Some(refs) => {
                let uses = refs.uses(icon_id);
                ui.label(format!("Used by {} row(s)", thousands(uses.len())));
                ui.separator();
                let row_height = ui.text_style_height(&egui::TextStyle::Button);
                if let Some(at) = nav.apply(uses.len()) {
                    let use_ = &uses[at];
                    followed = Some(route_of(refs.sheet_name(use_.sheet), use_));
                }
                let mut area = ScrollArea::vertical().auto_shrink(false);
                if let Some(offset) = nav.scroll(ui, row_height, uses.len()) {
                    area = area.vertical_scroll_offset(offset);
                }
                let output = area.show_rows(ui, row_height, uses.len(), |ui, range| {
                    for (at, use_) in uses[range.clone()].iter().enumerate() {
                        let sheet = refs.sheet_name(use_.sheet);
                        let row = use_row(ui, sheet, use_);
                        nav.mark(ui, range.start + at, row.response.rect);
                        if row.inner.clicked() {
                            followed = Some(route_of(sheet, use_));
                        }
                    }
                });
                nav.seen(&output);
            }
        }
        followed
    }
}

fn use_row(ui: &mut egui::Ui, sheet: &str, use_: &Use) -> egui::InnerResponse<egui::Response> {
    ui.horizontal(|ui| {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        let response = ui.add(
            egui::Label::new(RichText::new(sheet).color(ui.visuals().hyperlink_color))
                .sense(Sense::click()),
        );
        ui.label(RichText::new(use_.row.to_string()).weak());
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    })
}

fn route_of(sheet: &str, use_: &Use) -> String {
    if use_.subrow > 0 {
        format!("/sheet/{sheet}#R{}.{}", use_.row, use_.subrow)
    } else {
        format!("/sheet/{sheet}#R{}", use_.row)
    }
}

fn icon_source(
    icons: &IconManager,
    backend: &Backend,
    ctx: &egui::Context,
    path: &str,
) -> ManagedIcon {
    let excel = backend.excel().clone();
    icons.get_or_insert_icon(path, ctx, || {
        let path = path.to_owned();
        TrackedPromise::spawn_local(async move { excel.get_icon(&path).await })
    })
}

/// The icon's own dimensions, not the size it is drawn at.
fn pixel_size(ctx: &egui::Context, source: &egui::ImageSource<'static>) -> Option<[u32; 2]> {
    match source {
        egui::ImageSource::Texture(texture) => Some([texture.size.x as u32, texture.size.y as u32]),
        egui::ImageSource::Uri(uri) => {
            match ctx.try_load_image(uri, egui::SizeHint::Scale(1.0.into())) {
                Ok(egui::load::ImagePoll::Ready { image }) => {
                    Some([image.width() as u32, image.height() as u32])
                }
                _ => None,
            }
        }
        egui::ImageSource::Bytes { .. } => None,
    }
}

/// Draw an icon centered in `rect` at its own aspect. `Image::paint_at` fills whatever rect it is
/// given, which stretches everything that is not square.
fn fit_into(ui: &egui::Ui, source: egui::ImageSource<'static>, rect: egui::Rect) {
    let image = egui::Image::new(source).maintain_aspect_ratio(true);
    let size = image
        .load_and_calc_size(ui, rect.size())
        .unwrap_or(rect.size());
    image.paint_at(ui, egui::Rect::from_center_size(rect.center(), size));
}

fn checkerboard(ui: &egui::Ui, rect: egui::Rect) {
    const SQUARE: f32 = 8.0;
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, 0.0, Color32::from_gray(0x50));
    let mut y = rect.top();
    let mut row = 0;
    while y < rect.bottom() {
        let mut x = rect.left() + if row % 2 == 0 { 0.0 } else { SQUARE };
        while x < rect.right() {
            painter.rect_filled(
                egui::Rect::from_min_size(egui::pos2(x, y), Vec2::splat(SQUARE)),
                0.0,
                Color32::from_gray(0x38),
            );
            x += SQUARE * 2.0;
        }
        y += SQUARE;
        row += 1;
    }
}

fn thousands(n: usize) -> String {
    n.to_string()
        .as_bytes()
        .rchunks(3)
        .rev()
        .map(|group| std::str::from_utf8(group).unwrap())
        .join(",")
}
