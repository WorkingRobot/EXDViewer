use std::collections::{HashMap, HashSet};
use std::fmt;
use std::rc::Rc;

#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};
#[cfg(target_arch = "wasm32")]
use web_time::{Duration, Instant};

use anyhow::Result;
use egui::{
    Align, Button, CentralPanel, Color32, Label, Layout, Rect, RichText, ScrollArea, TextEdit,
    TextStyle, UiBuilder, Vec2, Widget, collapsing_header::paint_default_icon,
    containers::panel::Panel, pos2, vec2,
};
use nucleo_matcher::pattern::Pattern;

use crate::backend::Backend;
use crate::data::IconIndex;
use crate::excel::provider::ExcelProvider;
use crate::settings::api_base;
use crate::utils::{CollapsibleSidePanel, FuzzyMatcher, Side, TrackedPromise};

use pathlist::{PathList, Presence};

pub mod deps;
mod magic;
mod viewers;
use magic::Format;
use viewers::{Preview, Viewer};

/// Directories examined per frame while a search runs. Keeps the scan off the critical path without
/// making a full sweep of the corpus feel stalled.
const SCAN_BATCH: usize = 600;
/// Cap on search hits. Nobody scrolls past this, and it bounds the sort each frame.
const MAX_RESULTS: usize = 500;
/// How long a typed path has to stand still before the install is asked whether it holds it.\
const EXISTS_DELAY: Duration = Duration::from_millis(250);
/// Width the extension menu is held to.
const EXTENSION_MENU_WIDTH: f32 = 50.0;

/// One entry in the flattened view of the tree that is currently on screen.
enum Row {
    Dir {
        node: usize,
        depth: usize,
    },
    File {
        depth: usize,
        dir: usize,
        name: Rc<str>,
        /// Set for files absent from the path list: their position in the directory's index
        /// entries, which is where the real hash lives. `None` means the name came from the list.
        unnamed: Option<usize>,
    },
}

struct Node {
    segment: Box<str>,
    children: Vec<usize>,
    /// Index into [`PathList::dirs`] when this directory holds files of its own.
    dir: Option<usize>,
}

/// Sizes and durations in the log, so the console block stays readable at a glance.
pub struct Bytes(pub usize);
struct Millis(Duration);

impl fmt::Display for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            n if n >= 1 << 20 => write!(f, "{:.1} MiB", n as f64 / (1 << 20) as f64),
            n if n >= 1 << 10 => write!(f, "{:.0} KiB", n as f64 / (1 << 10) as f64),
            n => write!(f, "{n} B"),
        }
    }
}

impl fmt::Display for Millis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.1} ms", self.0.as_secs_f64() * 1000.0)
    }
}

/// Decode both payloads and build the tree, timing each stage. The whole thing runs on the frame
/// that the fetch lands, so anything slow here is a visible hitch rather than a background cost.
fn build_index(paths: &[u8], presence: &[u8]) -> Result<Loaded, String> {
    let at = Instant::now();
    let paths = PathList::decode(paths).map_err(|e| e.to_string())?;
    let paths_took = at.elapsed();

    let at = Instant::now();
    let presence = Presence::decode(presence).map_err(|e| e.to_string())?;
    let presence_took = at.elapsed();

    // The map is indexed by position in the list, so a pair from different builds would hide and
    // reveal the wrong files rather than fail.
    if paths.list_id() != presence.list_id() {
        return Err(format!(
            "This version's file map was built against path list {:016x}, but the list is {:016x}.",
            presence.list_id(),
            paths.list_id(),
        ));
    }

    let at = Instant::now();
    let mut live = live_dirs(&paths, &presence);
    let live_took = at.elapsed();

    let at = Instant::now();
    let (extra_dirs, unnamed, resolved) = place_unnamed(paths.dirs(), presence.unnamed());
    // Synthesised directories are only reachable through their unnamed files, so they are live by
    // definition; named directories that gained one may not have been live already.
    live.extend(paths.dirs().len()..paths.dirs().len() + extra_dirs.len());
    live.extend(
        unnamed
            .keys()
            .copied()
            .filter(|dir| *dir < paths.dirs().len()),
    );
    live.sort_unstable();
    live.dedup();
    let unnamed_took = at.elapsed();

    let at = Instant::now();
    let all_dirs: Vec<&str> = paths
        .dirs()
        .iter()
        .map(|d| &**d)
        .chain(extra_dirs.iter().map(|d| &**d))
        .collect();
    let (nodes, roots) = build_tree(&all_dirs, &live);
    let tree_took = at.elapsed();

    log::info!(
        "assets/decode: path list {} ({} dirs, {} paths), presence {} ({} present, {} unnamed)",
        Millis(paths_took),
        paths.dirs().len(),
        paths.len(),
        Millis(presence_took),
        presence.len(),
        presence.unnamed().len(),
    );
    log::info!(
        "assets/build: live dirs {} ({} kept), unnamed {} ({} placed in named dirs, {} in {} hash dirs), tree {} ({} nodes, {} roots)",
        Millis(live_took),
        live.len(),
        Millis(unnamed_took),
        resolved,
        presence.unnamed().len() - resolved,
        extra_dirs.len(),
        Millis(tree_took),
        nodes.len(),
        roots.len(),
    );
    log::info!(
        "assets/total: {} to first frame, {} resident",
        Millis(paths_took + presence_took + live_took + tree_took),
        Bytes(paths.resident_bytes()),
    );

    Ok(Loaded {
        paths,
        presence,
        nodes,
        roots,
        extra_dirs,
        unnamed,
        names: HashMap::new(),
    })
}

/// Directories with at least one path this version ships. A quarter of the global list belongs to
/// other versions, so building the tree from all of it would show directories that are entirely dead.
fn live_dirs(paths: &PathList, presence: &Presence) -> Vec<usize> {
    (0..paths.dirs().len())
        .filter(|dir| {
            let Ok(offset) = paths.name_offset(*dir) else {
                return false;
            };
            let count = paths.name_count(*dir).unwrap_or(0);
            (0..count).any(|i| presence.contains(offset + i))
        })
        .collect()
}

/// Tooltip and right-click menu for a game path: the path itself, the hashes the game's indexes key
/// it by, and a copy of each.
///
/// Only for paths that are really in the list. An unnamed file's path is synthesised around its
/// hash, so hashing it back would produce a confident-looking wrong answer.
/// Hover and right-click for a file. For an unnamed one the path is synthesised, so hashing it
/// would produce something the game never recorded; its actual index entry is used instead.
pub(crate) fn path_context(
    response: &egui::Response,
    path: &str,
    unnamed: Option<pathlist::Unnamed>,
) {
    use ironworks::sqpack::IndexHash;

    let (split, whole) = match unnamed {
        Some(file) if file.split => (Some(format!("{:016X}", file.hash)), None),
        Some(file) => (None, Some(format!("{:08X}", file.hash as u32))),
        None => {
            let (split, whole) = IndexHash::of(&path.to_lowercase());
            let IndexHash::Whole(whole) = whole else {
                unreachable!("of() always returns a whole hash")
            };
            (
                match split {
                    Some(IndexHash::Split(hash)) => Some(format!("{hash:016X}")),
                    _ => None,
                },
                Some(format!("{whole:08X}")),
            )
        }
    };

    response.clone().on_hover_ui(|ui| {
        ui.label(RichText::new(path).monospace());
        if unnamed.is_some() {
            ui.label(RichText::new("not in path list").weak());
        }
        ui.add_space(2.0);
        egui::Grid::new("path_hashes")
            .num_columns(2)
            .show(ui, |ui| {
                if let Some(split) = &split {
                    ui.label(RichText::new("index").weak());
                    ui.label(RichText::new(split).monospace());
                    ui.end_row();
                }
                if let Some(whole) = &whole {
                    ui.label(RichText::new("index2").weak());
                    ui.label(RichText::new(whole).monospace());
                    ui.end_row();
                }
            });
    });

    response.context_menu(|ui| {
        if ui.button("Copy").clicked() {
            ui.ctx().copy_text(path.to_owned());
            ui.close();
        }
        if let Some(split) = &split
            && ui.button("Copy (index hash)").clicked()
        {
            ui.ctx().copy_text(split.clone());
            ui.close();
        }
        if let Some(whole) = &whole
            && ui.button("Copy (index2 hash)").clicked()
        {
            ui.ctx().copy_text(whole.clone());
            ui.close();
        }
    });
}
/// The same hover and right-click a path gets, for a value the game identifies by a crc32: the name
/// on top, the hash in a labelled grid below it, and a copy of each.
pub(crate) fn crc_context(response: &egui::Response, kind: &str, name: &str, id: u32) {
    let hash = format!("{id:#010X}");

    response.clone().on_hover_ui(|ui| {
        ui.label(RichText::new(name).monospace());
        ui.label(RichText::new(kind).weak());
        ui.add_space(2.0);
        egui::Grid::new("crc_hash").num_columns(2).show(ui, |ui| {
            ui.label(RichText::new("crc32").weak());
            ui.label(RichText::new(&hash).monospace());
            ui.end_row();
        });
    });

    response.context_menu(|ui| {
        if ui.button("Copy").clicked() {
            ui.ctx().copy_text(name.to_owned());
            ui.close();
        }
        if ui.button("Copy (crc32)").clicked() {
            ui.ctx().copy_text(hash.clone());
            ui.close();
        }
    });
}

/// sqpack category ids, which are the first segment of every real path.
fn category_name(category: u8) -> Option<&'static str> {
    Some(match category {
        0x00 => "common",
        0x01 => "bgcommon",
        0x02 => "bg",
        0x03 => "cut",
        0x04 => "chara",
        0x05 => "shader",
        0x06 => "ui",
        0x07 => "sound",
        0x08 => "vfx",
        0x09 => "ui_script",
        0x0a => "exd",
        0x0b => "game_script",
        0x0c => "music",
        0x12 => "sqpack_test",
        0x13 => "debug",
        _ => return None,
    })
}

/// A listed directory's ancestors, for the hashes asked about, as `(directory, byte length)` so the
/// name can be sliced back out without owning it. Only walked when something failed to place, since
/// the large majority of unnamed files land on a directory that holds listed files of its own.
fn ancestors(dirs: &[Box<str>], wanted: &HashSet<u32>) -> HashMap<u32, (usize, usize)> {
    use ironworks::sqpack::IndexHash;

    let mut found = HashMap::new();
    if wanted.is_empty() {
        return found;
    }
    for (index, dir) in dirs.iter().enumerate() {
        let name = dir.to_ascii_lowercase();
        for (at, _) in name.match_indices('/') {
            let hash = IndexHash::directory(&name[..at]);
            if wanted.contains(&hash) {
                found.entry(hash).or_insert((index, at));
            }
        }
    }
    found
}

/// Give every unnamed file a home. The install records these only as hashes, but the directory half
/// of a split hash can be matched against the directories we do know, which lands the large majority
/// of them in their real folder. The rest fall back to a folder named for their directory hash.
///
/// Returns the synthesised directories, the per-directory file hashes, and how many were resolved.
fn place_unnamed(
    dirs: &[Box<str>],
    unnamed_files: &[pathlist::Unnamed],
) -> (Vec<Box<str>>, HashMap<usize, Vec<pathlist::Unnamed>>, usize) {
    use ironworks::sqpack::IndexHash;

    let mut by_hash: HashMap<u32, usize> = HashMap::with_capacity(dirs.len());
    for (index, dir) in dirs.iter().enumerate() {
        // The install is keyed on the lowercased path, and a few listed directories carry a capital.
        let name = dir.to_ascii_lowercase();
        let hash = IndexHash::directory(&name);
        // Some directories are listed under both spellings. Prefer the one already lowercase, so
        // which of the two nodes takes the files does not depend on iteration order.
        if name == **dir || !by_hash.contains_key(&hash) {
            by_hash.insert(hash, index);
        }
    }

    // `.index2` records a whole-path hash with no directory half, so there is nothing to match on;
    // none are present today, but they would have to go somewhere else.
    let split = || unnamed_files.iter().filter(|file| file.split);
    let unplaced: HashSet<u32> = split()
        .map(|file| (file.hash >> 32) as u32)
        .filter(|directory| !by_hash.contains_key(directory))
        .collect();
    let ancestors = ancestors(dirs, &unplaced);

    let mut extra_dirs: Vec<Box<str>> = Vec::new();
    let mut synthesised: HashMap<(u8, u8, u32), usize> = HashMap::new();
    let mut unnamed: HashMap<usize, Vec<pathlist::Unnamed>> = HashMap::new();
    let mut resolved = 0;

    for file in split() {
        let directory = (file.hash >> 32) as u32;
        let dir = match by_hash.get(&directory) {
            Some(known) => {
                resolved += 1;
                *known
            }
            None => {
                let key = (file.category, file.repository, directory);
                *synthesised.entry(key).or_insert_with(|| {
                    let name = match ancestors.get(&directory) {
                        // A directory holding nothing but other directories is not a key above, yet
                        // the tree already draws it, so its files belong there and not in a hash
                        // folder.
                        Some(&(index, length)) => dirs[index][..length].to_owned(),
                        None => {
                            let category = category_name(file.category)
                                .map(str::to_owned)
                                .unwrap_or_else(|| format!("category{:02x}", file.category));
                            let repository = match file.repository {
                                0 => "ffxiv".to_owned(),
                                n => format!("ex{n}"),
                            };
                            format!("{category}/{repository}/{directory:08x}")
                        }
                    };
                    extra_dirs.push(name.into_boxed_str());
                    dirs.len() + extra_dirs.len() - 1
                })
            }
        };
        unnamed.entry(dir).or_default().push(*file);
    }

    for files in unnamed.values_mut() {
        files.sort_unstable_by_key(|file| file.hash as u32);
    }
    (extra_dirs, unnamed, resolved)
}

fn build_tree(dirs: &[&str], live: &[usize]) -> (Vec<Node>, Vec<usize>) {
    let mut nodes: Vec<Node> = Vec::new();
    let mut roots: Vec<usize> = Vec::new();
    let mut lookup: HashMap<(Option<usize>, &str), usize> = HashMap::new();

    for dir_index in live.iter().copied() {
        let dir = dirs[dir_index];
        let mut parent = None;
        for segment in dir.split('/') {
            parent = Some(*lookup.entry((parent, segment)).or_insert_with(|| {
                nodes.push(Node {
                    segment: segment.into(),
                    children: Vec::new(),
                    dir: None,
                });
                let index = nodes.len() - 1;
                match parent {
                    Some(p) => nodes[p].children.push(index),
                    None => roots.push(index),
                }
                index
            }));
        }
        if let Some(node) = parent {
            nodes[node].dir = Some(dir_index);
        }
    }
    (nodes, roots)
}

/// Subdirectories first, then the directory's own files, matching how a file browser reads.
fn push_rows(
    loaded: &mut Loaded,
    expanded: &HashMap<usize, bool>,
    node: usize,
    depth: usize,
    rows: &mut Vec<Row>,
) {
    rows.push(Row::Dir { node, depth });
    if !expanded.get(&node).copied().unwrap_or(false) {
        return;
    }
    for child in loaded.nodes[node].children.clone() {
        push_rows(loaded, expanded, child, depth + 1, rows);
    }
    if let Some(dir) = loaded.nodes[node].dir {
        let names = loaded.names(dir);
        for (i, name) in names.all.iter().enumerate() {
            rows.push(Row::File {
                depth: depth + 1,
                dir,
                name: name.clone(),
                unnamed: i.checked_sub(names.named),
            });
        }
    }
}

/// A directory's file names: the ones from the path list, then the unnamed files as hashes.
struct Names {
    all: Vec<Rc<str>>,
    named: usize,
}

struct Loaded {
    paths: PathList,
    presence: Presence,
    nodes: Vec<Node>,
    roots: Vec<usize>,
    /// Directories that exist only because unnamed files hash into them. Indexed past the end of
    /// [`PathList::dirs`], so one index space covers both.
    extra_dirs: Vec<Box<str>>,
    /// The unnamed files each directory holds, keyed the same way. The full record is kept because
    /// reading one needs its repository, category and hash: it has no path to ask for.
    unnamed: HashMap<usize, Vec<pathlist::Unnamed>>,
    /// Names of directories the user has opened; only these are ever decoded.
    names: HashMap<usize, Rc<Names>>,
}

impl Loaded {
    /// The unnamed file a synthesised name refers to, so it can be read by hash.
    fn unnamed_file(&self, dir: usize, name: &str) -> Option<pathlist::Unnamed> {
        let hash = u32::from_str_radix(name, 16).ok()?;
        self.unnamed
            .get(&dir)?
            .iter()
            .find(|file| file.hash as u32 == hash)
            .copied()
    }

    /// Path of a directory, whether it came from the list or was synthesised for unnamed files.
    fn dir_path(&self, dir: usize) -> &str {
        let listed = self.paths.dirs().len();
        match dir.checked_sub(listed) {
            Some(extra) => &self.extra_dirs[extra],
            None => &self.paths.dirs()[dir],
        }
    }

    /// Names for a directory the user opened, kept so redrawing does not re-decode.
    fn unnamed_at(&self, dir: usize, index: usize) -> Option<pathlist::Unnamed> {
        self.unnamed.get(&dir)?.get(index).copied()
    }

    fn names(&mut self, dir: usize) -> Rc<Names> {
        if let Some(names) = self.names.get(&dir) {
            return names.clone();
        }
        // Unnamed files sit alongside the named ones, shown as their hash.
        let mut all: Vec<Rc<str>> = self.decode(dir).into_iter().map(Rc::from).collect();
        let named = all.len();
        if let Some(files) = self.unnamed.get(&dir) {
            all.extend(
                files
                    .iter()
                    .map(|f| Rc::from(format!("{:08x}", f.hash as u32))),
            );
        }
        let names = Rc::new(Names { all, named });
        self.names.insert(dir, names.clone());
        names
    }

    /// Names without caching. The search sweep touches every directory, so caching there would end
    /// up holding the whole corpus in memory.
    fn decode(&mut self, dir: usize) -> Vec<String> {
        let offset = match self.paths.name_offset(dir) {
            Ok(offset) => offset,
            Err(e) => {
                log::error!("No offset for directory {dir}: {e}");
                return Vec::new();
            }
        };
        let names = self.paths.names(dir).unwrap_or_else(|e| {
            log::error!("Failed to decode directory {dir}: {e}");
            Vec::new()
        });
        // The list is global, so anything this version does not ship is dropped here.
        names
            .into_iter()
            .enumerate()
            .filter(|(i, _)| self.presence.contains(offset + i))
            .map(|(_, name)| name)
            .collect()
    }
}

/// `T` is what the fetch yields, `R` what is kept once it is decoded.
enum Load<T: Send + 'static, R = T> {
    Idle,
    Loading(TrackedPromise<Result<T>>),
    Ready(R),
    Failed(String),
}

struct Scan {
    pattern: Pattern,
    /// The query as typed, lowercased. Empty when nothing but filter terms were given.
    query: String,
    /// The suffix a name has to end with, `.` included, or empty for no extension filter.
    suffix: String,
    literal: bool,
    cursor: usize,
    hits: Vec<(u32, String)>,
    /// Everything that matched, which outruns `hits` once the cap is reached.
    matched: usize,
    direct: Option<String>,
    exists: Load<bool>,
    typed: Instant,
}

/// What the browser wants the app to do after a frame.
pub enum Action {
    /// A file was picked; reflect it in the URL.
    Select(String),
    /// A handler wants to hand off to another tab.
    Navigate(String),
    /// A link named a file by a hash the list has since learned a name for. Replaces the URL rather
    /// than pushing, so going back does not land on the stale form and bounce forward again.
    Redirect(String),
}

/// What a path from a link or the lookup box turns out to name.
enum Revealed {
    /// Read it by path, whether or not the list names it: the install is keyed by the hash of the
    /// path either way.
    Path,
    /// A file the list does not name, read by the index entry the tree shows it under.
    Hash(pathlist::Unnamed),
    /// A hash the list has since learned a name for, so the link moves to the name.
    Renamed(String),
}

/// What a typed query asks for, once its filter terms have been read off the front.
struct Query {
    /// What is left to match on, which is empty when the query was nothing but filters.
    text: String,
    /// The suffix a name has to end with, `.` included, or empty for no extension filter.
    suffix: String,
    /// Whether to match the path itself rather than score it fuzzily.
    literal: bool,
}

/// Read the filter terms out of a query, leaving the rest to match on.
///
/// `ext:` is spelled the way the Everything search box spells it, and a query carrying a `/` is
/// taken to be part of a path rather than a fuzzy fragment, which is the same rule: nobody types a
/// separator into a fuzzy search, and typing `exd/` should leave that folder alone on screen.
fn parse_query(search: &str) -> Query {
    let mut suffix = String::new();
    let mut rest = Vec::new();
    for term in search.split_whitespace() {
        match term.strip_prefix("ext:") {
            Some(extension) => {
                let extension = extension.trim_start_matches('.');
                if !extension.is_empty() {
                    suffix = format!(".{}", extension.to_lowercase());
                }
            }
            None => rest.push(term),
        }
    }
    let text = rest.join(" ");
    Query {
        literal: text.contains('/'),
        text,
        suffix,
    }
}

/// Put `ext:extension` in the query, replacing whatever extension filter it already carried. It
/// leads so the rest of the query stays where the user left it.
fn set_extension(search: &str, extension: &str) -> String {
    let rest = search
        .split_whitespace()
        .filter(|term| !term.starts_with("ext:"))
        .collect::<Vec<_>>()
        .join(" ");
    match rest.is_empty() {
        true => format!("ext:{extension}"),
        false => format!("ext:{extension} {rest}"),
    }
}

/// `haystack.contains(needle)` without folding a copy of either. Game paths are ASCII, and folding
/// one per name would allocate more over a sweep than the matching itself costs.
///
/// `needle` is never empty: an empty query is answered before anything gets this far.
fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    let (haystack, needle) = (haystack.as_bytes(), needle.as_bytes());
    haystack.len() >= needle.len()
        && haystack
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle))
}

/// One line of the result tree: a folder holding matches, or a match itself.
enum Hit<'a> {
    Dir {
        path: &'a str,
        depth: usize,
        collapsed: bool,
    },
    File {
        path: &'a str,
        depth: usize,
        name: &'a str,
    },
}

/// Every directory a path passes through, outermost first, each as a whole path.
fn lineage(dir: &str) -> impl Iterator<Item = &str> {
    dir.match_indices('/')
        .map(move |(at, _)| &dir[..at])
        .chain(std::iter::once(dir))
}

/// Lay sorted matches out as the folders they sit in, so a search keeps the shape of the tree.
///
/// The rows are built from what matched rather than by pruning the real tree, which would mean
/// deciding which of six figures of directories hold a match before anything could be drawn.
fn group<'a>(paths: &[&'a str], collapsed: &HashSet<String>) -> Vec<Hit<'a>> {
    let mut rows = Vec::new();
    let mut open: Vec<&str> = Vec::new();
    for path in paths {
        let Some((dir, name)) = path.rsplit_once('/') else {
            continue;
        };
        let want: Vec<&str> = lineage(dir).collect();
        let common = open
            .iter()
            .zip(&want)
            .take_while(|(open, want)| open == want)
            .count();
        open.truncate(common);
        // A folder the user shut still gets its row, so it can be opened again; everything under it
        // is walked but not drawn.
        let mut hidden = open.iter().any(|dir| collapsed.contains(*dir));
        for (depth, dir) in want.iter().enumerate().skip(common) {
            open.push(dir);
            let shut = collapsed.contains(*dir);
            if !hidden {
                rows.push(Hit::Dir {
                    path: dir,
                    depth,
                    collapsed: shut,
                });
            }
            hidden |= shut;
        }
        if !hidden {
            rows.push(Hit::File {
                path,
                depth: want.len(),
                name,
            });
        }
    }
    rows
}

/// The listed name a synthesised one stands for.
fn named_by_hash(dir: &str, listed: &[Rc<str>], name: &str) -> Option<Rc<str>> {
    use ironworks::sqpack::IndexHash;

    if name.len() != 8 {
        return None;
    }
    let want = u32::from_str_radix(name, 16).ok()?;
    listed
        .iter()
        .find(|candidate| {
            matches!(
                IndexHash::of(&format!("{dir}/{candidate}").to_lowercase()).0,
                Some(IndexHash::Split(hash)) if hash as u32 == want
            )
        })
        .cloned()
}

/// The query read as a game path, so one the list does not carry can still be opened.
///
/// Every real path starts with a category directory and every listed name carries an extension, so
/// those two together separate a path from a fuzzy fragment such as `uld/mkd`. The install hashes a
/// lowercased path, so lowercasing loses nothing and makes the URL canonical.
fn direct_path(query: &str, roots: impl Fn(&str) -> bool) -> Option<String> {
    let query = query.trim().trim_matches('/').to_lowercase();
    // The query is handed straight to a URL, where a `#` would quietly truncate it. No character a
    // real path is spelled with falls outside this.
    if !query
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "_./-".contains(c))
    {
        return None;
    }
    let mut segments = query.split('/');
    (roots(segments.next()?) && segments.next_back()?.contains('.')).then_some(query)
}

pub struct AssetBrowser {
    state: Load<(Vec<u8>, Vec<u8>), Box<Loaded>>,
    /// The selected file as it was read: the kind of sqpack stream it was stored as, where the
    /// store reports one, and its raw bytes.
    bytes: Load<(Option<String>, Vec<u8>)>,
    bytes_of: Option<String>,
    /// What `bytes` turned out to hold, where its leading bytes say. Read once per selection.
    sniffed: Option<Format>,
    /// Rendered view of `bytes`, decoded once per selection.
    preview: Option<Preview>,
    /// Assets the current preview references, such as a material's textures.
    deps: deps::Deps,
    /// Set when the selection is an unnamed file, which has to be read by hash rather than by path.
    selected_unnamed: Option<pathlist::Unnamed>,
    /// Mipmap level on show, and the viewer picked from the dropdown, if not the recommended one.
    mip: u8,
    slice: u16,
    channels: Channels,
    viewer: Option<Viewer>,
    hex_page: usize,
    goto: Option<String>,
    search: String,
    /// Show matches in the folders they sit in rather than as one flat list.
    grouped: bool,
    scan: Option<Scan>,
    matcher: FuzzyMatcher,
    expanded: HashMap<usize, bool>,
    /// Folders the user collapsed in the results, keyed by path: the result tree is rebuilt from
    /// whatever matched, so it has no stable node indices to key on the way the full tree does.
    collapsed: HashSet<String>,
    selected: Option<String>,
    pending: Option<String>,
    redirect: Option<String>,
}

impl Default for AssetBrowser {
    fn default() -> Self {
        Self {
            state: Load::Idle,
            bytes: Load::Idle,
            bytes_of: None,
            sniffed: None,
            deps: deps::Deps::default(),
            preview: None,
            selected_unnamed: None,
            mip: 0,
            slice: 0,
            channels: Channels::default(),
            viewer: None,
            hex_page: 0,
            goto: None,
            search: String::new(),
            grouped: true,
            scan: None,
            matcher: FuzzyMatcher::new(),
            expanded: HashMap::new(),
            collapsed: HashSet::new(),
            selected: None,
            pending: None,
            redirect: None,
        }
    }
}

impl AssetBrowser {
    /// The file on show, or the one about to be once there is an index to place it in, so that
    /// entering the bare route restores the same URL either way.
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref().or(self.pending.as_deref())
    }

    /// Select the path from a deep link once the index is available.
    pub fn request(&mut self, path: String) {
        if self.selected.as_deref() != Some(path.as_str()) {
            self.pending = Some(path);
        }
    }

    /// Drop everything that came from the install, so a reconnect reads it all again.
    ///
    /// The selection becomes pending rather than being thrown away: the new install is asked for
    /// the same file, and whether it is named there is decided against its presence map, not the
    /// one that happened to be loaded when it was picked.
    pub fn reset(&mut self) {
        self.state = Load::Idle;
        self.bytes = Load::Idle;
        // Everything decoded from the bytes hangs off this, and `ensure_bytes` clears the lot when
        // it does not match the selection.
        self.bytes_of = None;
        self.deps = deps::Deps::default();
        self.selected_unnamed = None;
        self.scan = None;
        self.pending = self.pending.take().or(self.selected.take());
    }

    /// Apply a deep link, once there is an index to place it in.
    ///
    /// A link from the URL arrives on the frame the route changes, which on a cold load is a long
    /// way before the index has been fetched and decoded. It has to be held until then rather than
    /// consumed on the first frame, or reloading a page with a path in the URL selects nothing.
    fn apply_pending(&mut self) {
        match self.state {
            Load::Idle | Load::Loading(_) => {}
            Load::Ready(_) => {
                if let Some(pending) = self.pending.take() {
                    let (selected, unnamed) = match self.reveal(&pending) {
                        Revealed::Path => (pending, None),
                        Revealed::Hash(file) => (pending, Some(file)),
                        // The name arrived after the link was made, so the hash is no longer a file
                        // the tree shows; move the URL to the name it turned out to be.
                        Revealed::Renamed(named) => {
                            self.redirect = Some(named.clone());
                            (named, None)
                        }
                    };
                    self.selected_unnamed = unnamed;
                    self.selected = Some(selected);
                }
            }
            // Without an index the tree cannot expand to it, but the detail panel reads the file by
            // path, so the link still works and should not be thrown away.
            Load::Failed(_) => {
                if let Some(pending) = self.pending.take() {
                    self.selected_unnamed = None;
                    self.selected = Some(pending);
                }
            }
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, backend: &Backend) -> Option<Action> {
        self.poll(ui.ctx(), backend);
        self.apply_pending();
        let clicked = self.side_panel(ui, backend);
        let followed = self.detail_panel(ui, backend);
        let moved = self
            .goto
            .take()
            .map(Action::Navigate)
            .or_else(|| clicked.or(followed).map(Action::Select));
        // A redirect only restores a URL the app is already showing the right file for, so anything
        // the user did this frame supersedes it rather than firing over the top a frame later.
        let redirect = self.redirect.take().map(Action::Redirect);
        moved.or(redirect)
    }

    fn poll(&mut self, ctx: &egui::Context, backend: &Backend) {
        if matches!(self.state, Load::Idle) {
            let files = backend.files().clone();
            let api = api_base(ctx);
            self.state = Load::Loading(TrackedPromise::spawn_local(async move {
                // The list is version-independent and cached hard; the presence map is the only
                // per-version part, and it is a bit per path.
                let at = Instant::now();
                // Served prebuilt by the API; a local install builds its own from the same list.
                let (paths, presence) = files.path_index(&api).await?;
                log::info!(
                    "assets/fetch: path list {}, presence {}, in {}",
                    Bytes(paths.len()),
                    Bytes(presence.len()),
                    Millis(at.elapsed()),
                );
                Ok((paths, presence))
            }));
        }

        if let Load::Loading(promise) = &self.state
            && let Some(result) = promise.try_get()
        {
            self.state = match result.as_ref().map_err(|e| e.to_string()) {
                Ok((paths, presence)) => match build_index(paths, presence) {
                    Ok(loaded) => {
                        // Nowhere else holds the decoded list, so the icon subset is cut here.
                        backend.set_icons(IconIndex::build(&loaded.paths, &loaded.presence));
                        Load::Ready(Box::new(loaded))
                    }
                    Err(e) => Load::Failed(e),
                },
                Err(e) => Load::Failed(e),
            };
        }
    }

    /// Expand the tree down to `path` so a deep link lands somewhere visible, and report how the
    /// target has to be read.
    ///
    /// A path the tree cannot place still selects, since the install is asked by path and knows
    /// files the list does not name.
    fn reveal(&mut self, path: &str) -> Revealed {
        let Load::Ready(loaded) = &mut self.state else {
            return Revealed::Path;
        };
        let Some(cut) = path.rfind('/') else {
            return Revealed::Path;
        };
        let (folder, file) = (&path[..cut], &path[cut + 1..]);
        let mut parent: Option<usize> = None;
        for segment in folder.split('/') {
            let children: &[usize] = match parent {
                Some(p) => &loaded.nodes[p].children,
                None => &loaded.roots,
            };
            let Some(&next) = children
                .iter()
                .find(|&&c| &*loaded.nodes[c].segment == segment)
            else {
                return Revealed::Path;
            };
            self.expanded.insert(next, true);
            parent = Some(next);
        }
        let Some(dir) = parent.and_then(|node| loaded.nodes[node].dir) else {
            return Revealed::Path;
        };
        let names = loaded.names(dir);
        let listed = &names.all[..names.named];
        if listed.iter().any(|name| &**name == file) {
            return Revealed::Path;
        }
        // An unnamed file is shown as its hash, so its path is synthesised: it must not be hashed
        // back, and reading it has to go by index entry instead.
        if let Some(unnamed) = loaded.unnamed_file(dir, file) {
            return Revealed::Hash(unnamed);
        }
        match named_by_hash(folder, listed, file) {
            Some(named) => Revealed::Renamed(format!("{folder}/{named}")),
            None => Revealed::Path,
        }
    }

    fn side_panel(&mut self, ui: &mut egui::Ui, backend: &Backend) -> Option<String> {
        let mut clicked = None;
        CollapsibleSidePanel::new("asset_tree", Side::Left).show(ui, |ui, is_open| {
            if !is_open {
                return;
            }
            Panel::top("asset_tree_header").show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                        CollapsibleSidePanel::draw_arrow(ui, "asset_tree", Side::Left);
                        ui.vertical_centered_justified(|ui| ui.heading("Assets"));
                    });
                });
                ui.add_space(4.0);
                let mut restart = false;
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    if ui
                        .add_enabled(!self.search.is_empty(), Button::new("↩"))
                        .on_hover_text("Clear")
                        .clicked()
                    {
                        self.search.clear();
                        restart = true;
                    }
                    ui.toggle_value(&mut self.grouped, "🌳")
                        .on_hover_text("View as Tree");
                    let picked = parse_query(&self.search).suffix;
                    ui.menu_button("📄", |ui| {
                        ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                            // Three-letter names leave a menu too narrow to aim at, and too narrow
                            // for the scroll bar to sit clear of them.
                            ui.set_min_width(EXTENSION_MENU_WIDTH);
                            for (extension, what, _) in EXTENSIONS {
                                let on = picked.trim_start_matches('.') == *extension;
                                if ui
                                    .selectable_label(on, *extension)
                                    .on_hover_text(*what)
                                    .clicked()
                                {
                                    self.search = set_extension(&self.search, extension);
                                    restart = true;
                                    ui.close();
                                }
                            }
                        });
                    })
                    .response
                    .on_hover_text("Filter by extension");
                    restart |= ui
                        .add_sized(
                            Vec2::new(ui.available_width(), 0.0),
                            TextEdit::singleline(&mut self.search).hint_text("Search paths"),
                        )
                        .on_hover_text(
                            "ext:stm for one extension, or include a / to match the path itself",
                        )
                        .changed();
                });
                if restart {
                    self.scan = None;
                }
                ui.add_space(4.0);
            });

            CentralPanel::default().show(ui, |ui| match &mut self.state {
                Load::Idle | Load::Loading(_) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Loading path list…");
                    });
                }
                Load::Failed(error) => {
                    ui.colored_label(Color32::RED, error.clone());
                }
                Load::Ready(_) => {
                    clicked = if self.search.is_empty() {
                        self.scan = None;
                        self.draw_tree(ui)
                    } else {
                        self.draw_search(ui, backend)
                    };
                }
            });
        });
        clicked
    }

    /// Flatten the expanded parts of the tree into the rows actually on screen, so the list can be
    /// virtualised even though the corpus has six figures of directories.
    fn visible_rows(&mut self) -> Vec<Row> {
        let Load::Ready(loaded) = &mut self.state else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        let roots = loaded.roots.clone();
        for node in roots {
            push_rows(loaded, &self.expanded, node, 0, &mut rows);
        }
        rows
    }

    fn draw_tree(&mut self, ui: &mut egui::Ui) -> Option<String> {
        let rows = self.visible_rows();
        let Load::Ready(loaded) = &self.state else {
            return None;
        };
        let mut clicked = None;
        let mut toggle = None;
        let row_height = ui.text_style_height(&TextStyle::Button);
        // The label indents with spaces, so the triangle has to be placed in the same units.
        let space_width =
            ui.fonts_mut(|f| f.glyph_width(&TextStyle::Button.resolve(ui.style()), ' '));
        let icon_width = ui.spacing().icon_width;
        ScrollArea::vertical().auto_shrink(false).show_rows(
            ui,
            row_height,
            rows.len(),
            |ui, range| {
                ui.with_layout(Layout::top_down_justified(Align::Min), |ui| {
                    for row in &rows[range] {
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                        match row {
                            Row::Dir { node, depth } => {
                                let expanded = self.expanded.get(node).copied().unwrap_or(false);
                                // The indent leaves room for the triangle, which is painted rather
                                // than written: the bundled fonts have no small-triangle glyph, so a
                                // text arrow comes out as a tofu box.
                                let text = RichText::new(format!(
                                    "{}    {}",
                                    "    ".repeat(*depth),
                                    loaded.nodes[*node].segment
                                ));
                                let named = loaded.nodes[*node]
                                    .dir
                                    .is_none_or(|dir| dir < loaded.paths.dirs().len());
                                let row = Button::selectable(
                                    false,
                                    if named { text } else { text.weak() },
                                )
                                .ui(ui);
                                let icon = Rect::from_center_size(
                                    pos2(
                                        row.rect.left()
                                            + space_width * 4.0 * *depth as f32
                                            + icon_width / 2.0,
                                        row.rect.center().y,
                                    ),
                                    Vec2::splat(icon_width),
                                );
                                paint_default_icon(
                                    ui,
                                    if expanded { 1.0 } else { 0.0 },
                                    &row.clone().with_new_rect(icon),
                                );
                                if row.clicked() {
                                    toggle = Some((*node, !expanded));
                                }
                            }
                            Row::File {
                                depth,
                                dir,
                                name,
                                unnamed,
                            } => {
                                let path = format!("{}/{}", loaded.dir_path(*dir), name);
                                let text =
                                    RichText::new(format!("{}{}", "    ".repeat(*depth), name));
                                let selected = self.selected.as_deref() == Some(path.as_str());
                                let text = if unnamed.is_some() { text.weak() } else { text };
                                let response = Button::selectable(selected, text).ui(ui);
                                path_context(
                                    &response,
                                    &path,
                                    unnamed.and_then(|index| loaded.unnamed_at(*dir, index)),
                                );
                                if response.clicked() {
                                    clicked = Some(path);
                                }
                            }
                        }
                    }
                });
            },
        );
        if let Some((node, expanded)) = toggle {
            self.expanded.insert(node, expanded);
        }
        clicked
    }

    fn draw_search(&mut self, ui: &mut egui::Ui, backend: &Backend) -> Option<String> {
        self.advance_scan(ui.ctx(), backend);
        let Some(scan) = &self.scan else {
            return None;
        };
        let Load::Ready(loaded) = &self.state else {
            return None;
        };
        let total = loaded.paths.dirs().len();
        let mut clicked = None;
        // Offered above the matches, because someone typing a whole path already knows what they
        // want and the sweep for it takes a moment.
        if let Some(path) = scan.direct.clone() {
            ui.label(RichText::new("Open by path").weak());
            let offer = match &scan.exists {
                Load::Idle | Load::Loading(_) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Checking…");
                    });
                    false
                }
                Load::Ready(found) => {
                    if !found {
                        ui.label(RichText::new("This version has no such file.").weak());
                    }
                    *found
                }
                // A check that could not run is not evidence of absence, so the path is still
                // offered and the reason is put where the user can see it.
                Load::Failed(error) => {
                    ui.label(RichText::new(format!("Could not check: {error}")).weak());
                    true
                }
            };
            if offer {
                ui.with_layout(Layout::top_down_justified(Align::Min), |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                    let selected = self.selected.as_deref() == Some(path.as_str());
                    if Button::selectable(selected, path.as_str())
                        .ui(ui)
                        .on_hover_text(
                            "Read this path from the install, whether or not the list names it",
                        )
                        .clicked()
                    {
                        clicked = Some(path);
                    }
                });
            }
            ui.separator();
        }

        let scanning = scan.cursor < total;
        if scanning {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!(
                    "Searching… {}%",
                    scan.cursor.saturating_mul(100) / total.max(1)
                ));
            });
        } else if scan.hits.is_empty() {
            ui.label("No matches.");
        } else {
            // Count everything that matched, not only what is shown.
            ui.label(
                RichText::new(if scan.matched > scan.hits.len() {
                    format!("{} of {} matches", scan.hits.len(), scan.matched)
                } else {
                    format!(
                        "{} match{}",
                        scan.matched,
                        if scan.matched == 1 { "" } else { "es" }
                    )
                })
                .weak(),
            );
        }

        let row_height = ui.text_style_height(&egui::TextStyle::Button);
        if !self.grouped {
            ScrollArea::vertical().auto_shrink(false).show_rows(
                ui,
                row_height,
                scan.hits.len(),
                |ui, range| {
                    ui.with_layout(Layout::top_down_justified(Align::Min), |ui| {
                        for (_, path) in &scan.hits[range] {
                            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                            let selected = self.selected.as_deref() == Some(path.as_str());
                            if Button::selectable(selected, path.as_str())
                                .ui(ui)
                                .on_hover_text(path)
                                .clicked()
                            {
                                clicked = Some(path.clone());
                            }
                        }
                    });
                },
            );
            return clicked;
        }

        // Score order is what ranks a flat list, but a tree only reads as one in path order.
        let mut paths: Vec<&str> = scan.hits.iter().map(|(_, path)| path.as_str()).collect();
        paths.sort_unstable();
        let rows = group(&paths, &self.collapsed);

        let mut toggle = None;
        let space_width =
            ui.fonts_mut(|f| f.glyph_width(&TextStyle::Button.resolve(ui.style()), ' '));
        let icon_width = ui.spacing().icon_width;
        ScrollArea::vertical().auto_shrink(false).show_rows(
            ui,
            row_height,
            rows.len(),
            |ui, range| {
                ui.with_layout(Layout::top_down_justified(Align::Min), |ui| {
                    for row in &rows[range] {
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                        match row {
                            Hit::Dir {
                                path,
                                depth,
                                collapsed,
                            } => {
                                let segment = path.rsplit('/').next().unwrap_or(path);
                                let text = RichText::new(format!(
                                    "{}    {segment}",
                                    "    ".repeat(*depth)
                                ));
                                let response = Button::selectable(false, text).ui(ui);
                                let icon = Rect::from_center_size(
                                    pos2(
                                        response.rect.left()
                                            + space_width * 4.0 * *depth as f32
                                            + icon_width / 2.0,
                                        response.rect.center().y,
                                    ),
                                    Vec2::splat(icon_width),
                                );
                                paint_default_icon(
                                    ui,
                                    if *collapsed { 0.0 } else { 1.0 },
                                    &response.clone().with_new_rect(icon),
                                );
                                if response.clicked() {
                                    toggle = Some((*path, !*collapsed));
                                }
                            }
                            Hit::File { path, depth, name } => {
                                let text =
                                    RichText::new(format!("{}{name}", "    ".repeat(*depth)));
                                let selected = self.selected.as_deref() == Some(*path);
                                if Button::selectable(selected, text)
                                    .ui(ui)
                                    .on_hover_text(*path)
                                    .clicked()
                                {
                                    clicked = Some((*path).to_owned());
                                }
                            }
                        }
                    }
                });
            },
        );

        if let Some((path, collapsed)) = toggle {
            if collapsed {
                self.collapsed.insert(path.to_owned());
            } else {
                self.collapsed.remove(path);
            }
        }
        clicked
    }

    fn advance_scan(&mut self, ctx: &egui::Context, backend: &Backend) {
        let Load::Ready(loaded) = &mut self.state else {
            return;
        };
        let scan = self.scan.get_or_insert_with(|| {
            let query = parse_query(&self.search);
            Scan {
                pattern: FuzzyMatcher::parse_pattern(&query.text),
                literal: query.literal,
                query: query.text.to_lowercase(),
                suffix: query.suffix,
                cursor: 0,
                hits: Vec::new(),
                matched: 0,
                direct: direct_path(&query.text, |root| {
                    loaded
                        .roots
                        .iter()
                        .any(|&node| &*loaded.nodes[node].segment == root)
                }),
                exists: Load::Idle,
                typed: Instant::now(),
            }
        });

        if let Some(path) = &scan.direct {
            let settled = match &scan.exists {
                Load::Idle if scan.typed.elapsed() >= EXISTS_DELAY => {
                    let path = path.clone();
                    let files = backend.files().clone();
                    Some(Load::Loading(TrackedPromise::spawn_local(async move {
                        Ok(files
                            .exists_many(&[path])
                            .await?
                            .first()
                            .copied()
                            .unwrap_or(false))
                    })))
                }
                Load::Idle => {
                    ctx.request_repaint_after(EXISTS_DELAY);
                    None
                }
                Load::Loading(promise) => promise.try_get().map(|result| {
                    match result.as_ref().map_err(|e| e.to_string()) {
                        Ok(exists) => Load::Ready(*exists),
                        Err(error) => Load::Failed(error),
                    }
                }),
                Load::Ready(_) | Load::Failed(_) => None,
            };
            if let Some(settled) = settled {
                scan.exists = settled;
            }
        }

        let total = loaded.paths.dirs().len();
        if scan.cursor >= total {
            return;
        }

        let end = (scan.cursor + SCAN_BATCH).min(total);
        for dir in scan.cursor..end {
            let dir_path = loaded.paths.dirs()[dir].clone();
            for name in loaded.decode(dir) {
                // Cheapest test first: an extension rules a name out without building its path or
                // scoring it, which is what keeps an extension-only sweep of the whole list quick.
                // Compared as bytes because folding the case of every one of a million-odd names
                // would allocate more than the rest of the sweep put together; a path is ASCII.
                let tail = name.len().checked_sub(scan.suffix.len());
                if !tail.is_some_and(|at| {
                    name.as_bytes()[at..].eq_ignore_ascii_case(scan.suffix.as_bytes())
                }) {
                    continue;
                }
                let path = format!("{dir_path}/{name}");
                // An empty query is every file the filters left, which is what `ext:stm` on its own
                // has to mean. The fuzzy matcher scores nothing against an empty pattern, so asking
                // it here would answer that with no matches at all.
                let score = if scan.query.is_empty() {
                    Some(0)
                } else if scan.literal {
                    contains_ignore_ascii_case(&path, &scan.query).then_some(0)
                } else {
                    self.matcher
                        .score_one(&scan.pattern, &path)
                        .map(|score| score.get())
                };
                if let Some(score) = score {
                    scan.matched += 1;
                    scan.hits.push((score, path));
                }
            }
        }
        scan.cursor = end;
        scan.hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scan.hits.truncate(MAX_RESULTS);
        ctx.request_repaint();
    }

    fn detail_panel(&mut self, ui: &mut egui::Ui, backend: &Backend) -> Option<String> {
        // A material links through to the textures it binds, so the panel can ask for a new
        // selection the same way the tree does.
        let mut follow = None;
        CentralPanel::default().show(ui, |ui| {
            let Some(path) = self.selected.clone() else {
                if CollapsibleSidePanel::is_collapsed(ui.ctx(), "asset_tree") {
                    ui.horizontal(|ui| {
                        CollapsibleSidePanel::draw_arrow(ui, "asset_tree", Side::Left)
                    });
                }
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new("Select a file to inspect.").weak());
                });
                return;
            };

            self.ensure_bytes(ui, backend, &path);

            let (stream, empty) = match &self.bytes {
                Load::Ready((kind, bytes)) => {
                    let size = Bytes(bytes.len());
                    let label = match kind {
                        Some(kind) => format!("{kind} ({size})"),
                        None => size.to_string(),
                    };
                    (Some(label), bytes.is_empty())
                }
                _ => (None, false),
            };

            Panel::top("asset_header").show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if CollapsibleSidePanel::is_collapsed(ui.ctx(), "asset_tree") {
                        ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
                            CollapsibleSidePanel::draw_arrow(ui, "asset_tree", Side::Left);
                        });
                    }
                    ui.vertical_centered_justified(|ui| ui.heading(crate::utils::file_name(&path)));
                });
                ui.add_space(4.0);
                // Wrapped in a `horizontal` so the row is sized by its content. A bare `with_layout`
                // would take the panel's remaining height, which the panel derives from its content:
                // the row would then grow by a few pixels on every repaint.
                ui.horizontal(|ui| {
                    let row = ui.max_rect();
                    let left = ui
                        .scope(|ui| {
                            if let Some(stream) = &stream {
                                ui.label(RichText::new(stream).weak());
                            }
                        })
                        .response
                        .rect;

                    let right = ui
                        .with_layout(Layout::right_to_left(Align::Center), |ui| {
                            // A file with no data has nothing for any viewer to show, so there is
                            // nothing to choose between.
                            if !empty {
                                self.viewer_picker(ui, &path);
                            }
                            match Kind::of(&path) {
                                Kind::Sheet => {
                                    if let Some(sheet) =
                                        sheet_name(backend.excel().get_entries(), &path)
                                        && ui.button(format!("Open “{sheet}” in Sheets")).clicked()
                                    {
                                        self.goto = Some(format!("/sheet/{sheet}"));
                                    }
                                }
                                Kind::SheetList => {
                                    if ui.button("Open the Sheets tab").clicked() {
                                        self.goto = Some("/sheet".to_string());
                                    }
                                }
                                Kind::Other => {}
                            }
                        })
                        .response
                        .rect;

                    // Centered on the row rather than on the gap the two sides leave, which is off
                    // center whenever they differ in width. Never wide enough to reach either of
                    // them, so a long path truncates rather than running underneath one.
                    let font = TextStyle::Body.resolve(ui.style());
                    let width = ui
                        .painter()
                        .layout_no_wrap(path.clone(), font, Color32::PLACEHOLDER)
                        .size()
                        .x;
                    let room = (row.center().x - left.right()).min(right.left() - row.center().x)
                        - ui.spacing().item_spacing.x;
                    let flanks = left.union(right);
                    let band = Rect::from_center_size(
                        pos2(row.center().x, flanks.center().y),
                        vec2(width.min(room * 2.0).max(0.0), flanks.height()),
                    );
                    ui.scope_builder(
                        UiBuilder::new()
                            .max_rect(band)
                            .layout(Layout::left_to_right(Align::Center)),
                        |ui| {
                            let label = ui.add(
                                Label::new(RichText::new(&path).weak())
                                    .truncate()
                                    .sense(egui::Sense::click()),
                            );
                            path_context(&label, &path, self.selected_unnamed);
                        },
                    );
                });
                ui.add_space(4.0);
            });

            // Only textures and images have anything to put in the sidebar.
            if self.preview.as_ref().is_some_and(Preview::has_details) {
                let mut change = None;
                CollapsibleSidePanel::new("asset_info", Side::Right).show(ui, |ui, is_open| {
                    if !is_open {
                        return;
                    }
                    Panel::top("asset_info_header").show(ui, |ui| {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            // Mirror of the tree panel: the arrow goes against this panel's outer
                            // edge, which is the left one, and the heading centers in the rest.
                            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                CollapsibleSidePanel::draw_arrow(ui, "asset_info", Side::Right);
                                ui.vertical_centered_justified(|ui| ui.heading("Details"));
                            });
                        });
                        ui.add_space(4.0);
                    });
                    CentralPanel::default().show(ui, |ui| {
                        if let Some(preview) = &self.preview {
                            change = preview.info_ui(
                                ui,
                                (self.mip, self.slice, self.channels),
                                &mut follow,
                                &mut self.deps,
                                backend,
                            );
                        }
                    });
                });
                if let Some((mip, slice, channels)) = change {
                    // The slice is chosen at draw time, so only the settings that change the pixels
                    // are worth throwing the decoded preview away for.
                    let redecode = (mip, channels) != (self.mip, self.channels);
                    self.mip = mip;
                    self.slice = slice;
                    self.channels = channels;
                    if redecode {
                        self.preview = None;
                    }
                }
            }

            let showing = self.viewer.unwrap_or(self.recommended(&path));
            CentralPanel::default().show(ui, |ui| {
                if CollapsibleSidePanel::is_collapsed(ui.ctx(), "asset_info")
                    && self.preview.as_ref().is_some_and(Preview::has_details)
                {
                    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                        CollapsibleSidePanel::draw_arrow(ui, "asset_info", Side::Right);
                    });
                }
                match &self.bytes {
                    Load::Idle | Load::Loading(_) => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Reading file…");
                        });
                    }
                    Load::Failed(e) => {
                        ui.colored_label(Color32::RED, e.clone());
                    }
                    Load::Ready((_, bytes)) if bytes.is_empty() => {
                        ui.centered_and_justified(|ui| {
                            ui.label(RichText::new("This file is empty").weak());
                        });
                    }
                    Load::Ready((_, bytes)) if showing == Viewer::Raw => {
                        let mut page = self.hex_page;
                        hex_dump(ui, bytes, &mut page);
                        self.hex_page = page;
                    }
                    Load::Ready((_, bytes)) => {
                        if let Some(preview) = &self.preview
                            && let Some(target) =
                                preview.ui(ui, bytes, self.slice, &mut self.deps, backend)
                        {
                            follow = Some(target);
                        }
                    }
                }
            });
        });
        follow
    }

    /// What a file is shown with unless the dropdown says otherwise. The bytes are taken over the
    /// name wherever they say anything, which is the only thing an unnamed file has to go on.
    fn recommended(&self, path: &str) -> Viewer {
        self.sniffed
            .map_or_else(|| Viewer::from_extension(path), Format::viewer)
    }

    /// The viewer dropdown, which throws the decoded preview away whenever the choice changes. Where
    /// the bytes and the extension disagree, the extension's reading stays on offer below the
    /// recommendation rather than being dropped.
    fn viewer_picker(&mut self, ui: &mut egui::Ui, path: &str) {
        let extension = Viewer::from_extension(path);
        let recommended = self.recommended(path);
        let named = self
            .sniffed
            .map_or_else(|| recommended.label(), Format::label);
        // Following the recommendation reads as whatever the file turned out to be, so a sheet page
        // does not sit closed as the `Bytes` it is shown with.
        let chosen = match self.viewer {
            Some(viewer) => viewer.label(),
            None => named,
        };
        // ComboBox closes itself on click; the arms below never call `close`.
        egui::ComboBox::from_id_salt("asset_viewer")
            .selected_text(chosen)
            .show_ui(ui, |ui| {
                let mut pick = |ui: &mut egui::Ui, viewer: Option<Viewer>, label: String| {
                    if ui.selectable_label(self.viewer == viewer, label).clicked() {
                        self.viewer = viewer;
                        self.preview = None;
                    }
                };
                pick(ui, None, format!("{named} (Recommended)"));
                // Only where the name claims something of its own, and something else: an
                // unrecognized extension has nothing to say that `Bytes` below does not.
                if extension != recommended && extension != Viewer::Raw {
                    pick(
                        ui,
                        Some(extension),
                        format!("{} (Extension)", extension.label()),
                    );
                }
                pick(ui, Some(Viewer::Raw), Viewer::Raw.label().to_owned());
                ui.separator();
                for viewer in Viewer::RENDERED {
                    // The recommended one is already the entry at the top. It stays in the list,
                    // disabled, so every viewer keeps the same place.
                    if viewer == recommended {
                        ui.add_enabled(false, Button::selectable(false, viewer.described()));
                    } else {
                        pick(ui, Some(viewer), viewer.described());
                    }
                }
            });
    }

    /// Fetch the selected file if it is not already in hand, and decode a view of it.
    fn ensure_bytes(&mut self, ui: &mut egui::Ui, backend: &Backend, path: &str) {
        if self.bytes_of.as_deref() != Some(path) {
            self.bytes_of = Some(path.to_string());
            self.sniffed = None;
            self.preview = None;
            self.mip = 0;
            self.slice = 0;
            self.channels = Channels::default();
            self.viewer = None;
            self.hex_page = 0;
            let files = backend.files().clone();
            // An unnamed file has no path to ask for, so it is fetched by hash instead.
            let unnamed = self.selected_unnamed;
            let wanted = path.to_string();
            self.bytes = Load::Loading(TrackedPromise::spawn_local(async move {
                let at = Instant::now();
                let (kind, bytes) = match unnamed {
                    Some(file) => {
                        files
                            .read_stream_by_hash(
                                file.repository,
                                file.category,
                                file.hash,
                                file.split,
                            )
                            .await?
                    }
                    None => files.read_stream(&wanted).await?,
                };
                log::info!(
                    "assets/read: {wanted} {} in {}",
                    Bytes(bytes.len()),
                    Millis(at.elapsed())
                );
                Ok((kind, bytes))
            }));
        }
        if let Load::Loading(promise) = &self.bytes
            && let Some(result) = promise.try_get()
        {
            self.bytes = match result.as_ref() {
                Ok((kind, bytes)) => {
                    self.sniffed = magic::sniff(bytes);
                    Load::Ready((kind.clone(), bytes.clone()))
                }
                Err(e) => Load::Failed(e.to_string()),
            };
        }

        // Decoding uploads a texture, so it needs the context and happens here rather than in the
        // fetch. Once per file, or again when a different mipmap is picked.
        let viewer = self.viewer.unwrap_or(self.recommended(path));
        if let Load::Ready((_, bytes)) = &self.bytes
            && !bytes.is_empty()
            && self.preview.is_none()
            && viewer != Viewer::Raw
        {
            let at = Instant::now();
            let preview = Preview::decode(ui.ctx(), path, bytes, viewer, self.mip, self.channels);
            log::info!(
                "assets/preview: {} in {}",
                viewer.label(),
                Millis(at.elapsed())
            );
            self.preview = Some(preview);
        }
    }

    fn sheet_shortcut(&mut self, ui: &mut egui::Ui, backend: &Backend, path: &str) {
        let sheet = sheet_name(backend.excel().get_entries(), path);
        ui.vertical_centered(|ui| match sheet {
            Some(sheet) => {
                ui.label("Excel sheet data.");
                ui.add_space(8.0);
                if ui.button(format!("Open “{sheet}” in Sheets")).clicked() {
                    self.goto = Some(format!("/sheet/{sheet}"));
                }
            }
            None => {
                ui.label("Excel sheet data.");
                ui.add_space(8.0);
                ui.label(RichText::new("Couldn't work out which sheet this belongs to.").weak());
            }
        });
    }
}

/// Whether a file is reachable from the Sheets tab, which is the only thing its extension decides
/// here. What it holds is [`EXTENSIONS`].
enum Kind {
    Sheet,
    SheetList,
    Other,
}

impl Kind {
    fn of(path: &str) -> Self {
        match path.rsplit('.').next().unwrap_or_default() {
            "exd" | "exh" => Kind::Sheet,
            "exl" => Kind::SheetList,
            _ => Kind::Other,
        }
    }
}

/// Every extension the path list carries, with what it holds. Also the menu the search box offers,
/// so the order is the order they are listed in.
const EXTENSIONS: &[(&str, &str, Viewer)] = &[
    ("exd", "Excel sheet data", Viewer::Raw),
    ("exh", "Excel sheet header", Viewer::Raw),
    ("exl", "Excel sheet list", Viewer::Raw),
    ("tex", "Texture", Viewer::Texture),
    ("atex", "Animated texture", Viewer::Texture),
    ("png", "PNG image", Viewer::Image),
    ("mdl", "Model", Viewer::Raw),
    ("mtrl", "Material", Viewer::Material),
    ("shpk", "Shader package", Viewer::Shpk),
    ("shcd", "Shader code", Viewer::Shcd),
    ("scd", "Sound container", Viewer::Raw),
    ("ggd", "Grass grid data", Viewer::Raw),
    ("pcb", "Player collision binary", Viewer::Raw),
    ("sklb", "Skeleton", Viewer::Raw),
    ("skp", "Skeleton parameters", Viewer::Skp),
    ("pap", "Animation", Viewer::Raw),
    ("tmb", "Animation timeline", Viewer::Raw),
    ("phyb", "Physics bones", Viewer::Raw),
    ("eid", "Bone bindings", Viewer::Eid),
    ("atch", "Attachment points", Viewer::Atch),
    ("avfx", "Animated VFX", Viewer::Raw),
    ("uld", "UI layout", Viewer::Uld),
    ("lgb", "Layer group, a zone's placed objects", Viewer::Raw),
    (
        "sgb",
        "Shared group, a reusable set of objects",
        Viewer::Raw,
    ),
    ("lvb", "Level variable binary", Viewer::Raw),
    ("svb", "Sky visibility binary", Viewer::Raw),
    ("uwb", "Zone bounds or environment volume", Viewer::Raw),
    ("envb", "Environment binary", Viewer::Raw),
    ("lcb", "Light culling binary", Viewer::Raw),
    ("obsb", "Object behavior set binary", Viewer::Raw),
    ("essb", "Environment sound scrape binary", Viewer::Raw),
    ("luab", "Lua bytecode", Viewer::Luab),
    ("cutb", "Cutscene", Viewer::Raw),
    ("imc", "Image change data", Viewer::Imc),
    ("eqdp", "Equipment deformer parameters", Viewer::Raw),
    ("eqp", "Equipment parameters", Viewer::Raw),
    ("gmp", "Gimmick parameters", Viewer::Raw),
    ("est", "Equipment skeleton template", Viewer::Est),
    ("evp", "Equipment VFX parameters", Viewer::Raw),
    ("pbd", "Bone deformers", Viewer::Raw),
    ("amb", "Ambient set placement", Viewer::Raw),
    ("tera", "Terrain", Viewer::Tera),
    ("hwc", "Handware cursor", Viewer::Raw),
    ("fdt", "Font data table", Viewer::Font),
    ("gfd", "Graphics font data", Viewer::Icons),
    ("stm", "Stain map", Viewer::Stm),
    ("cmp", "Character make parameters", Viewer::Raw),
    ("plt", "PAP load table", Viewer::Raw),
];

/// `exd/item_0_en.exd` -> `Item`, `exd/content/foo_0_en.exd` -> `content/Foo`.
///
/// Most sheet names are nested (`content/DeepDungeon2Achievement`), so the candidate is built from
/// the path below `exd/` rather than the file name alone. The game appends a row offset and a
/// language to each page, so trailing `_parts` are dropped one at a time, longest candidate first,
/// and only within the final segment — sheet names contain underscores of their own.
fn sheet_name(entries: &HashMap<String, i32>, path: &str) -> Option<String> {
    let relative = path.strip_prefix("exd/").unwrap_or(path);
    let stem = relative.rsplit_once('.').map_or(relative, |(head, _)| head);
    let split = stem.rfind('/').map_or(0, |i| i + 1);

    let mut candidate = stem;
    loop {
        if let Some((name, _)) = entries
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(candidate))
        {
            return Some(name.clone());
        }
        match candidate[split..].rfind('_') {
            Some(i) => candidate = &candidate[..split + i],
            None => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(names: &[&str]) -> HashMap<String, i32> {
        names.iter().map(|n| ((*n).to_string(), 0)).collect()
    }

    #[test]
    fn resolves_sheet_names_from_exd_paths() {
        let sheets = entries(&[
            "Item",
            "Quest",
            "content/DeepDungeon2Achievement",
            "quest/000/Quest_00000",
        ]);
        for (path, want) in [
            ("exd/item_0_en.exd", Some("Item")),
            ("exd/item.exh", Some("Item")),
            (
                "exd/content/deepdungeon2achievement_0_en.exd",
                Some("content/DeepDungeon2Achievement"),
            ),
            // the longest candidate wins, so a page of a nested sheet does not fall back to `Quest`
            (
                "exd/quest/000/quest_00000_en.exd",
                Some("quest/000/Quest_00000"),
            ),
            ("exd/nosuchsheet_0_en.exd", None),
        ] {
            assert_eq!(
                sheet_name(&sheets, path).as_deref(),
                want,
                "resolving {path}"
            );
        }
    }

    #[test]
    fn builds_intermediate_tree_levels() {
        // `bg` and `bg/ffxiv` hold no files of their own and are never listed, but the tree needs them.
        let dirs = ["bg/ffxiv/sea_s1", "bg/ffxiv/wil_w1", "exd"];
        let live: Vec<usize> = (0..dirs.len()).collect();
        let (nodes, roots) = build_tree(&dirs, &live);
        assert_eq!(roots.len(), 2, "bg and exd are the roots");

        let bg = roots
            .iter()
            .copied()
            .find(|&n| &*nodes[n].segment == "bg")
            .unwrap();
        assert!(nodes[bg].dir.is_none(), "bg holds no files itself");
        let ffxiv = nodes[bg].children[0];
        assert_eq!(&*nodes[ffxiv].segment, "ffxiv");
        assert_eq!(nodes[ffxiv].children.len(), 2);
        for child in &nodes[ffxiv].children {
            assert!(
                nodes[*child].dir.is_some(),
                "leaf directories map to a dir index"
            );
        }

        let exd = roots
            .iter()
            .copied()
            .find(|&n| &*nodes[n].segment == "exd")
            .unwrap();
        assert_eq!(nodes[exd].dir, Some(2));
    }

    /// An unnamed file whose directory hash matches a known directory belongs in that directory,
    /// not in a hash folder. Only genuinely unknown directories get synthesised.
    #[test]
    fn unnamed_files_land_in_their_real_directory_when_it_is_known() {
        use ironworks::sqpack::IndexHash;

        let dirs: Vec<Box<str>> = [
            "common/savedata",
            "music/ffxiv",
            // The list carries some directories with a capital, and a few under both spellings.
            "sound/voice/Vo_Emote",
            "sound/voice/Vo_Line",
            "sound/voice/vo_line",
            // Nothing is listed directly in common/graphics, only below it.
            "common/graphics/texture",
        ]
        .iter()
        .map(|d| (*d).into())
        .collect();

        let entry = |directory: &str, file: u64| pathlist::Unnamed {
            repository: 0,
            category: 0x00,
            hash: (u64::from(IndexHash::directory(directory)) << 32) | file,
            split: true,
        };

        let unnamed = [
            // hashes into "common/savedata", which the list knows
            entry("common/savedata", 0xdead_beef),
            // a directory nothing in the list hashes to
            pathlist::Unnamed {
                repository: 4,
                category: 0x0c,
                hash: (0x1234_5678u64 << 32) | 0x0000_00ff,
                split: true,
            },
            // the install hashes the lowercased name, whatever spelling the list recorded
            entry("sound/voice/vo_emote", 0x0000_0001),
            entry("sound/voice/vo_line", 0x0000_0002),
            // a directory the list only ever mentions as the parent of another
            entry("common/graphics", 0x0000_0003),
            entry("common/graphics", 0x0000_0004),
        ];
        let (extra_dirs, placed, resolved) = place_unnamed(&dirs, &unnamed);
        assert_eq!(
            resolved, 3,
            "the ancestor is not a directory the list holds"
        );
        assert_eq!(
            &*extra_dirs,
            &["music/ex4/12345678".into(), "common/graphics".into()],
            "an unknown directory is named for its hash, a known ancestor for itself"
        );

        let at = |name: &str| dirs.iter().position(|d| &**d == name).unwrap();
        assert_eq!(placed[&at("common/savedata")], vec![unnamed[0]]);
        assert_eq!(placed[&dirs.len()], vec![unnamed[1]]);
        assert_eq!(placed[&at("sound/voice/Vo_Emote")], vec![unnamed[2]]);
        assert_eq!(
            placed[&at("sound/voice/vo_line")],
            vec![unnamed[3]],
            "the lowercase spelling wins over the capitalised duplicate"
        );
        assert_eq!(
            placed[&(dirs.len() + 1)],
            vec![unnamed[4], unnamed[5]],
            "both files share the one synthesised common/graphics"
        );
    }

    /// A path in the URL arrives many frames before the index has loaded. Holding it until then is
    /// the whole point; an earlier version consumed it on the first frame and the link was lost.
    #[test]
    fn a_deep_link_survives_until_the_index_is_ready() {
        let mut browser = AssetBrowser::default();
        browser.request("exd/root.exl".to_string());

        for _ in 0..3 {
            browser.apply_pending();
            assert_eq!(
                browser.pending.as_deref(),
                Some("exd/root.exl"),
                "still fetching, so the link must be kept"
            );
            assert!(browser.selected.is_none());
        }

        browser.state = Load::Failed("no api".to_string());
        browser.apply_pending();
        assert_eq!(browser.selected.as_deref(), Some("exd/root.exl"));
        assert!(browser.pending.is_none(), "applied exactly once");
    }

    /// The list is global, so directories belonging only to other versions must not reach the tree.
    #[test]
    fn omits_directories_absent_from_this_version() {
        let dirs = ["bg/ffxiv/sea_s1", "music/ex9", "exd"];
        let (nodes, roots) = build_tree(&dirs, &[0, 2]);
        assert!(
            !nodes.iter().any(|n| &*n.segment == "music"),
            "a dead directory should leave no node behind, not even an empty branch"
        );
        assert_eq!(roots.len(), 2);
        let dirs_mapped: Vec<Option<usize>> = nodes.iter().map(|n| n.dir).collect();
        assert!(dirs_mapped.contains(&Some(0)) && dirs_mapped.contains(&Some(2)));
        assert!(!dirs_mapped.contains(&Some(1)));
    }

    /// An extension on its own has to match every file carrying it. The fuzzy matcher scores
    /// nothing against an empty pattern, so leaving the query to it answered `ext:stm` with no
    /// matches unless something else was typed too.
    #[test]
    fn an_extension_on_its_own_leaves_nothing_to_match_on() {
        let query = parse_query("ext:stm");
        assert_eq!(query.suffix, ".stm");
        assert!(
            query.text.is_empty(),
            "the filter term is not left to match on"
        );
        assert!(!query.literal);
    }

    /// Picking from the menu is a replacement rather than another term, so picking twice does not
    /// leave a query no file can satisfy.
    #[test]
    fn picking_an_extension_replaces_the_one_already_there() {
        assert_eq!(set_extension("", "tex"), "ext:tex");
        assert_eq!(set_extension("chara", "tex"), "ext:tex chara");
        assert_eq!(set_extension("ext:stm chara", "tex"), "ext:tex chara");
        assert_eq!(set_extension("chara ext:stm", "tex"), "ext:tex chara");
        assert_eq!(parse_query(&set_extension("ext:stm", "tex")).suffix, ".tex");
    }

    /// [`EXTENSIONS`] is the one place an extension is named, so a viewer offering one the table
    /// does not list would be unreachable, and a name listed twice would shadow itself.
    #[test]
    fn every_viewer_is_reachable_from_the_extension_table() {
        let mut seen = HashSet::new();
        for (extension, ..) in EXTENSIONS {
            assert!(seen.insert(extension), "{extension} is listed twice");
        }
        for viewer in Viewer::RENDERED {
            let mut extensions = viewer.extensions().peekable();
            // Text is reached by reading a file rather than by its name: nothing the game still
            // ships carries a text extension.
            assert!(
                extensions.peek().is_some() || matches!(viewer, Viewer::Text),
                "{} reads nothing the table lists",
                viewer.label()
            );
            for extension in extensions {
                assert!(
                    Viewer::from_extension(&format!("a/b.{extension}")) == viewer,
                    "{extension} does not come back to the viewer that claims it"
                );
            }
        }
    }

    #[test]
    fn filter_terms_come_out_of_the_query_wherever_they_sit() {
        let query = parse_query("ext:.STM chara");
        assert_eq!(
            (query.suffix.as_str(), query.text.as_str()),
            (".stm", "chara")
        );

        // A separator is what tells a path apart from a fuzzy fragment, so `uld/mkd` is a path and
        // `mkd` is not.
        assert!(parse_query("bg/ffxiv").literal);
        assert!(!parse_query("terrain").literal);
        assert!(parse_query("exd/ ext:exh").literal);
    }

    #[test]
    fn a_path_matches_anywhere_in_it_whatever_its_case() {
        assert!(contains_ignore_ascii_case(
            "chara/xls/attachOffset/d1040.atch",
            "attachoffset"
        ));
        assert!(contains_ignore_ascii_case("exd/root.exl", "exd/"));
        assert!(!contains_ignore_ascii_case("exd/root.exl", "exdx"));
        assert!(
            !contains_ignore_ascii_case("ab", "abc"),
            "a needle longer than the haystack should not index past it"
        );
    }

    /// Rows as `depth:name`, so a layout reads the way it is drawn.
    fn laid_out(paths: &[&str], collapsed: &[&str]) -> Vec<String> {
        let shut = collapsed.iter().map(|dir| (*dir).to_owned()).collect();
        group(paths, &shut)
            .iter()
            .map(|row| match row {
                Hit::Dir { path, depth, .. } => {
                    format!("{depth}:{}/", path.rsplit('/').next().unwrap())
                }
                Hit::File { depth, name, .. } => format!("{depth}:{name}"),
            })
            .collect()
    }

    #[test]
    fn matches_keep_the_folders_they_sit_in() {
        let rows = laid_out(
            &[
                "bg/ffxiv/fst_f1/bgplate/terrain.tera",
                "bg/ffxiv/sea_s1/bgplate/terrain.tera",
                "chara/base_material/stainingtemplate.stm",
            ],
            &[],
        );
        assert_eq!(
            rows,
            [
                "0:bg/",
                "1:ffxiv/",
                "2:fst_f1/",
                "3:bgplate/",
                "4:terrain.tera",
                // Only the part that differs is reopened; `bg/ffxiv` is not drawn twice.
                "2:sea_s1/",
                "3:bgplate/",
                "4:terrain.tera",
                "0:chara/",
                "1:base_material/",
                "2:stainingtemplate.stm",
            ]
        );
    }

    #[test]
    fn a_shut_folder_keeps_its_row_and_hides_what_is_under_it() {
        let paths = [
            "bg/ffxiv/fst_f1/bgplate/terrain.tera",
            "bg/ffxiv/sea_s1/bgplate/terrain.tera",
            "chara/base_material/stainingtemplate.stm",
        ];
        let rows = laid_out(&paths, &["bg/ffxiv"]);
        assert_eq!(
            rows,
            [
                "0:bg/",
                "1:ffxiv/",
                "0:chara/",
                "1:base_material/",
                "2:stainingtemplate.stm"
            ],
            "the shut folder should still be there to reopen, and nothing below it drawn"
        );
    }
}

/// Text is capped because the hex dump below it already covers the whole file, and a multi-megabyte
/// label is not something egui should be asked to lay out.
pub const MAX_TEXT_PREVIEW: usize = 256 * 1024;

/// Which color channels of an image to show. Masking them off is how a packed texture (normal
/// maps, masks, occlusion) is read: the interesting data is rarely the RGB composite.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Channels {
    r: bool,
    g: bool,
    b: bool,
    a: bool,
}

impl Default for Channels {
    fn default() -> Self {
        Self {
            r: true,
            g: true,
            b: true,
            a: true,
        }
    }
}

impl Channels {
    fn all(self) -> bool {
        self.r && self.g && self.b && self.a
    }

    /// Zero the unselected channels, or, when exactly one is picked, show it as grayscale so a
    /// single packed channel is actually readable.
    fn apply(self, image: &mut image::RgbaImage) {
        if self.all() {
            return;
        }
        let only = match (self.r, self.g, self.b, self.a) {
            (true, false, false, false) => Some(0),
            (false, true, false, false) => Some(1),
            (false, false, true, false) => Some(2),
            (false, false, false, true) => Some(3),
            _ => None,
        };
        for pixel in image.pixels_mut() {
            let [r, g, b, a] = pixel.0;
            pixel.0 = match only {
                Some(channel) => {
                    let value = pixel.0[channel];
                    [value, value, value, u8::MAX]
                }
                // Alpha is forced opaque when deselected, so the color channels stay visible.
                None => [
                    if self.r { r } else { 0 },
                    if self.g { g } else { 0 },
                    if self.b { b } else { 0 },
                    if self.a { a } else { u8::MAX },
                ],
            };
        }
    }
}

/// Which renderer to show a file with. `Raw` is always available; the rest only make sense for the
/// formats they understand, but any of them can be forced from the dropdown.
const HEX_COLS: usize = 16;
/// Rows per page of the byte view. egui positions a virtualised list in `f32`, which stops being
/// exact past ~16.7M pixels, so a big enough file would scroll unevenly or fail to reach its end.
/// One page is 1 MiB, comfortably inside that, and files below it get no pagination at all.
const HEX_PAGE_ROWS: usize = 64 * 1024;

/// Offset, hex, ASCII. Rows are virtualised, so only what is on screen is ever formatted.
fn hex_dump(ui: &mut egui::Ui, bytes: &[u8], page: &mut usize) {
    use std::fmt::Write as _;

    let rows = bytes.len().div_ceil(HEX_COLS);
    let pages = rows.div_ceil(HEX_PAGE_ROWS).max(1);
    *page = (*page).min(pages - 1);

    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{} ({} bytes)", Bytes(bytes.len()), bytes.len())).weak());
        if pages > 1 {
            ui.separator();
            if ui.add_enabled(*page > 0, Button::new("◀")).clicked() {
                *page -= 1;
            }
            ui.label(format!("page {} / {pages}", *page + 1));
            if ui
                .add_enabled(*page + 1 < pages, Button::new("▶"))
                .clicked()
            {
                *page += 1;
            }
            ui.label(
                RichText::new(format!("from {:#010X}", *page * HEX_PAGE_ROWS * HEX_COLS)).weak(),
            );
        }
    });
    ui.add_space(4.0);

    let first_row = *page * HEX_PAGE_ROWS;
    let page_rows = (rows - first_row).min(HEX_PAGE_ROWS);
    let row_height = ui.text_style_height(&TextStyle::Monospace);
    ScrollArea::both()
        .auto_shrink(false)
        .id_salt(*page)
        .show_rows(ui, row_height, page_rows, |ui, range| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            let mut line = String::with_capacity(80);
            for row in range {
                let start = (first_row + row) * HEX_COLS;
                let chunk = &bytes[start..(start + HEX_COLS).min(bytes.len())];
                line.clear();
                let _ = write!(line, "{start:08X}  ");
                for i in 0..HEX_COLS {
                    if i == HEX_COLS / 2 {
                        line.push(' ');
                    }
                    match chunk.get(i) {
                        Some(b) => {
                            let _ = write!(line, "{b:02X} ");
                        }
                        None => line.push_str("   "),
                    }
                }
                line.push(' ');
                line.extend(chunk.iter().map(|b| match b {
                    0x20..=0x7e => *b as char,
                    _ => '.',
                }));
                ui.label(RichText::new(&line).monospace());
            }
        });
}
