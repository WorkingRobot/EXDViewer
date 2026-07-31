use std::{
    collections::{HashMap, HashSet},
    hash::Hasher,
    io::Read,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use bytes::Bytes;
use flate2::read::GzDecoder;
use ironworks::sqpack::IndexHash;
use mini_moka::sync::{Cache, CacheBuilder};
use tokio::sync::{Mutex, RwLock};
use twox_hash::XxHash64;
use xiv_core::file::version::GameVersion;

use crate::{
    config::PathList as PathListConfig,
    data::{GameData, Target},
};

pub struct MasterList {
    dirs: Vec<Box<str>>,
    names: Vec<Box<str>>,
    /// Index into `names` where each directory's run starts, plus a terminating entry.
    starts: Vec<u32>,
    id: u64,
}

impl MasterList {
    fn parse(sources: &[String]) -> Self {
        let mut by_dir: HashMap<&str, Vec<&str>> = HashMap::new();
        for line in sources.iter().flat_map(|source| source.lines()) {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (dir, name) = match line.rfind('/') {
                Some(i) => (&line[..i], &line[i + 1..]),
                None => ("", line),
            };
            by_dir.entry(dir).or_default().push(name);
        }

        let mut by_dir: Vec<(&str, Vec<&str>)> = by_dir.into_iter().collect();
        by_dir.sort_unstable_by_key(|(dir, _)| *dir);

        let mut dirs: Vec<Box<str>> = Vec::with_capacity(by_dir.len());
        let mut names: Vec<Box<str>> = Vec::new();
        let mut starts = Vec::with_capacity(by_dir.len() + 1);
        for (dir, mut run) in by_dir {
            starts.push(names.len() as u32);
            run.sort_unstable();
            run.dedup();
            names.extend(run.into_iter().map(Into::into));
            dirs.push(dir.into());
        }
        starts.push(names.len() as u32);

        let mut digest = XxHash64::with_seed(0);
        for text in dirs.iter().chain(&names) {
            digest.write(text.as_bytes());
            digest.write(&[0]);
        }
        let id = digest.finish();

        Self {
            dirs,
            names,
            starts,
            id,
        }
    }

    /// Borrowed view in the shape the encoder wants.
    fn entries(&self) -> Vec<(&str, Vec<&str>)> {
        self.iter()
            .map(|(dir, names)| (dir, names.iter().map(|name| &**name).collect()))
            .collect()
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn dir_count(&self) -> usize {
        self.dirs.len()
    }

    pub fn path_count(&self) -> usize {
        self.names.len()
    }

    fn iter(&self) -> impl Iterator<Item = (&str, &[Box<str>])> {
        self.dirs.iter().enumerate().map(|(i, dir)| {
            let range = self.starts[i] as usize..self.starts[i + 1] as usize;
            (&**dir, &self.names[range])
        })
    }
}

struct Freshness {
    etags: HashMap<String, String>,
    checked_at: Instant,
}

/// A source's body, or nothing when it has not changed since the etag we hold.
enum Fetched {
    Unchanged,
    Body(String, Option<String>),
}

pub struct PathIndex {
    config: PathListConfig,
    client: reqwest::Client,
    list: RwLock<Option<Arc<MasterList>>>,
    freshness: Mutex<Option<Freshness>>,
    global: RwLock<Option<(Arc<MasterList>, Bytes)>>,
    presence: Cache<(u64, Target, GameVersion), Bytes>,
}

impl std::fmt::Debug for PathIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PathIndex")
            .field("url", &self.config.url)
            .finish_non_exhaustive()
    }
}

impl PathIndex {
    pub fn new(config: PathListConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
            list: RwLock::new(None),
            freshness: Mutex::new(None),
            global: RwLock::new(None),
            presence: CacheBuilder::new(4096).build(),
        }
    }

    /// One source's body. Only ResLogger's ships gzipped; an extra list is plain text.
    async fn fetch(&self, url: &str, etag: Option<&str>, packed: bool) -> Result<Fetched> {
        let mut request = self.client.get(url);
        if let Some(etag) = etag {
            request = request.header(reqwest::header::IF_NONE_MATCH, etag);
        }
        let response = request.send().await?.error_for_status()?;
        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(Fetched::Unchanged);
        }

        let etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let body = response.bytes().await?;
        log::info!("Fetched {url} ({} bytes)", body.len());

        let text = tokio::task::spawn_blocking(move || match packed {
            true => {
                let mut text = String::new();
                GzDecoder::new(&body[..])
                    .read_to_string(&mut text)
                    .context("failed to decompress path list")?;
                anyhow::Ok(text)
            }
            false => anyhow::Ok(String::from_utf8(body.to_vec())?),
        })
        .await??;
        Ok(Fetched::Body(text, etag))
    }

    async fn master(&self) -> Result<Arc<MasterList>> {
        let mut freshness = self.freshness.lock().await;
        let ttl = Duration::from_secs(self.config.ttl_minutes * 60);
        let cached = self.list.read().await.clone();
        let stale = freshness
            .as_ref()
            .is_none_or(|f| f.checked_at.elapsed() >= ttl);
        if !stale && let Some(list) = &cached {
            return Ok(list.clone());
        }

        let urls: Vec<&str> = std::iter::once(self.config.url.as_str())
            .chain(self.config.extra_urls.iter().map(String::as_str))
            .collect();
        let mut bodies: Vec<Option<String>> = vec![None; urls.len()];
        let mut unchanged: Vec<usize> = Vec::new();
        let mut etags: HashMap<String, String> = HashMap::new();

        // Two passes, because a source that answers 304 leaves us without its text and we keep only
        // etags between refreshes. The second pass only runs when something actually moved.
        for pass in 0..2 {
            let wanted: Vec<usize> = match pass {
                0 => (0..urls.len()).collect(),
                _ if unchanged.len() == urls.len() => break,
                _ => std::mem::take(&mut unchanged),
            };
            for index in wanted {
                let url = urls[index];
                let known = (pass == 0)
                    .then(|| freshness.as_ref().and_then(|f| f.etags.get(url)))
                    .flatten()
                    .filter(|_| cached.is_some())
                    .cloned();
                match self.fetch(url, known.as_deref(), index == 0).await {
                    Ok(Fetched::Unchanged) => {
                        unchanged.push(index);
                        if let Some(etag) = known {
                            etags.insert(url.to_owned(), etag);
                        }
                    }
                    Ok(Fetched::Body(text, etag)) => {
                        bodies[index] = Some(text);
                        if let Some(etag) = etag {
                            etags.insert(url.to_owned(), etag);
                        }
                    }
                    // An extra source only adds to the list. Rebuilding without one would change
                    // the list id and discard every presence map built against it, so a cached list
                    // is served on instead and the extra is picked up at the next refresh.
                    Err(error) if index > 0 => {
                        log::error!("Could not fetch extra path list {url}: {error}");
                        if let Some(list) = cached.clone() {
                            if let Some(f) = freshness.as_mut() {
                                f.checked_at = Instant::now();
                            }
                            return Ok(list);
                        }
                        log::error!("No list is cached, so building without {url}");
                    }
                    Err(error) => return Err(error),
                }
            }
            if pass == 0 && unchanged.len() == urls.len() {
                let list = cached.context("every source answered 304 with no list cached")?;
                log::info!("Path lists unchanged upstream");
                if let Some(f) = freshness.as_mut() {
                    f.checked_at = Instant::now();
                }
                return Ok(list);
            }
        }

        let bodies: Vec<String> = bodies.into_iter().flatten().collect();
        anyhow::ensure!(!bodies.is_empty(), "no path list source produced a body");

        let list = tokio::task::spawn_blocking(move || MasterList::parse(&bodies)).await?;
        log::info!(
            "Parsed path list: {} paths across {} directories",
            list.path_count(),
            list.dir_count()
        );

        let list = Arc::new(list);
        *self.list.write().await = Some(list.clone());
        *freshness = Some(Freshness {
            etags,
            checked_at: Instant::now(),
        });
        Ok(list)
    }

    pub async fn list_id(&self) -> Result<u64> {
        Ok(self.master().await?.id())
    }

    pub async fn global(&self) -> Result<(u64, Bytes)> {
        let master = self.master().await?;
        let id = master.id();
        if let Some((cached_for, blob)) = &*self.global.read().await
            && Arc::ptr_eq(cached_for, &master)
        {
            return Ok((id, blob.clone()));
        }

        let built = {
            let master = master.clone();
            tokio::task::spawn_blocking(move || {
                let entries = master.entries();
                log::info!("Encoding global path list: {} directories", entries.len());
                pathlist::compress(&pathlist::encode(&entries, master.id()))
            })
            .await??
        };
        let blob = Bytes::from(built);
        *self.global.write().await = Some((master, blob.clone()));
        Ok((id, blob))
    }

    pub async fn presence(
        &self,
        data: &GameData,
        target: Target,
        version: GameVersion,
        wanted: u64,
    ) -> Result<Option<Bytes>> {
        let master = self.master().await?;
        let list_id = master.id();
        if list_id != wanted {
            return Ok(None);
        }
        let key = (list_id, target, version.clone());
        if let Some(cached) = self.presence.get(&key) {
            return Ok(Some(cached));
        }
        if let Some(stored) = self.read_stored(&key).await {
            self.presence.insert(key, stored.clone());
            return Ok(Some(stored));
        }

        data.warm_indexes(target, version.clone()).await?;
        let ironworks = data.get_version(target, version.clone()).await?;

        let map = tokio::task::block_in_place(|| {
            let mut installed: HashSet<(u8, u8, IndexHash)> = HashSet::new();
            for package in ironworks.resources() {
                let entries = package.entries()?;
                log::info!("Enumerated {} installed files", entries.len());
                for entry in entries {
                    installed.insert((entry.repository, entry.category, entry.hash));
                }
            }

            let resource = ironworks
                .resources()
                .first()
                .context("no sqpack resource")?;
            let map = pathlist::build_presence(
                master.path_count(),
                &installed,
                |path| resource.locate(path).ok(),
                list_id,
                |visit| {
                    for (dir, names) in master.iter() {
                        for name in names {
                            visit(dir, name);
                        }
                    }
                },
            );

            log::info!(
                "{target}/{version}: {} listed paths, {} installed files",
                master.path_count(),
                installed.len()
            );

            anyhow::Ok(pathlist::compress(&map)?)
        })?;

        let map = Bytes::from(map);
        self.write_stored(&key, &map).await;
        self.presence.insert(key, map.clone());
        Ok(Some(map))
    }

    fn stored_path(
        &self,
        (list_id, target, version): &(u64, Target, GameVersion),
    ) -> Option<PathBuf> {
        let format = pathlist::PRESENCE_VERSION;
        self.config.cache_directory.as_ref().map(|dir| {
            dir.join(format!(
                "{list_id:016x}-{}-{version}.pdb{format}",
                target.file_key()
            ))
        })
    }

    async fn read_stored(&self, key: &(u64, Target, GameVersion)) -> Option<Bytes> {
        let path = self.stored_path(key)?;
        match tokio::fs::read(&path).await {
            Ok(bytes) => {
                log::info!("Loaded presence map from {}", path.display());
                Some(Bytes::from(bytes))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                log::warn!("Could not read {}: {e}", path.display());
                None
            }
        }
    }

    async fn write_stored(&self, key: &(u64, Target, GameVersion), map: &Bytes) {
        let Some(path) = self.stored_path(key) else {
            return;
        };
        if let Some(parent) = path.parent()
            && let Err(e) = tokio::fs::create_dir_all(parent).await
        {
            log::warn!("Could not create {}: {e}", parent.display());
            return;
        }
        let temporary = path.with_extension("exdb.partial");
        if let Err(e) = tokio::fs::write(&temporary, map).await {
            log::warn!("Could not write {}: {e}", temporary.display());
            return;
        }
        if let Err(e) = tokio::fs::rename(&temporary, &path).await {
            log::warn!("Could not store {}: {e}", path.display());
        } else {
            log::info!("Stored presence map at {}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(sources: &[&str]) -> MasterList {
        MasterList::parse(&sources.iter().map(|s| (*s).to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn list_id_distinguishes_a_shifted_split() {
        let a = parse(&["dir/ab\ndir/c\n"]);
        let b = parse(&["dir/a\ndir/bc\n"]);
        assert_eq!(a.path_count(), b.path_count());
        assert_ne!(a.id(), b.id());
    }

    #[test]
    fn list_id_ignores_input_order() {
        let a = parse(&["b/two\na/one\n"]);
        let b = parse(&["a/one\nb/two\n"]);
        assert_eq!(a.id(), b.id());
    }

    /// Extra sources are unioned, so which one carries a path cannot change the list.
    #[test]
    fn extra_sources_merge_without_regard_to_order_or_overlap() {
        let together = parse(&["a/one\nb/two\n"]);

        assert_eq!(parse(&["a/one\n", "b/two\n"]).id(), together.id());
        assert_eq!(parse(&["b/two\n", "a/one\n"]).id(), together.id());
        assert_eq!(
            parse(&["a/one\nb/two\n", "b/two\n"]).id(),
            together.id(),
            "a path both sources carry is listed once"
        );
        assert_eq!(together.path_count(), 2);
    }

    /// The extra list is hand-maintained, so it has to tolerate comments and blank lines.
    #[test]
    fn comments_are_not_paths() {
        let list = parse(&["# a note\n\na/one\n   # indented\n"]);
        assert_eq!(list.path_count(), 1);
        assert_eq!(list.id(), parse(&["a/one\n"]).id());
    }
}
