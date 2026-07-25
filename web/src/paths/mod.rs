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
use xiv_core::file::{slug::Slug, version::GameVersion};

use crate::{config::PathList as PathListConfig, data::GameData};

pub struct MasterList {
    dirs: Vec<Box<str>>,
    names: Vec<Box<str>>,
    /// Index into `names` where each directory's run starts, plus a terminating entry.
    starts: Vec<u32>,
    id: u64,
}

impl MasterList {
    fn parse(text: &str) -> Self {
        let mut by_dir: HashMap<&str, Vec<&str>> = HashMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
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
    etag: Option<String>,
    checked_at: Instant,
}

pub struct PathIndex {
    config: PathListConfig,
    client: reqwest::Client,
    list: RwLock<Option<Arc<MasterList>>>,
    freshness: Mutex<Option<Freshness>>,
    global: RwLock<Option<(Arc<MasterList>, Bytes)>>,
    presence: Cache<(u64, Slug, GameVersion), Bytes>,
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

        let mut request = self.client.get(&self.config.url);
        if let Some(etag) = freshness
            .as_ref()
            .and_then(|f| f.etag.as_deref())
            .filter(|_| cached.is_some())
        {
            request = request.header(reqwest::header::IF_NONE_MATCH, etag);
        }
        let response = request.send().await?.error_for_status()?;

        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            let list = cached.context("upstream returned 304 with no list cached")?;
            log::info!("Path list unchanged upstream");
            if let Some(f) = freshness.as_mut() {
                f.checked_at = Instant::now();
            }
            return Ok(list);
        }

        let etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let gz = response.bytes().await?;

        log::info!("Fetched path list ({} bytes compressed)", gz.len());
        let list = tokio::task::spawn_blocking(move || {
            let mut text = String::new();
            GzDecoder::new(&gz[..])
                .read_to_string(&mut text)
                .context("failed to decompress path list")?;
            anyhow::Ok(MasterList::parse(&text))
        })
        .await??;
        log::info!(
            "Parsed path list: {} paths across {} directories",
            list.path_count(),
            list.dir_count()
        );

        let list = Arc::new(list);
        *self.list.write().await = Some(list.clone());
        *freshness = Some(Freshness {
            etag,
            checked_at: Instant::now(),
        });
        Ok(list)
    }

    pub async fn global(&self) -> Result<Bytes> {
        let master = self.master().await?;
        if let Some((cached_for, blob)) = &*self.global.read().await
            && Arc::ptr_eq(cached_for, &master)
        {
            return Ok(blob.clone());
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
        Ok(blob)
    }

    pub async fn presence(
        &self,
        data: &GameData,
        slug: Slug,
        version: GameVersion,
    ) -> Result<Bytes> {
        let master = self.master().await?;
        let list_id = master.id();
        let key = (list_id, slug, version.clone());
        if let Some(cached) = self.presence.get(&key) {
            return Ok(cached);
        }
        if let Some(stored) = self.read_stored(&key).await {
            self.presence.insert(key, stored.clone());
            return Ok(stored);
        }

        data.warm_indexes(slug, version.clone()).await?;
        let ironworks = data.get_version(slug, version.clone()).await?;

        let map = tokio::task::block_in_place(|| {
            let mut installed: HashSet<(u8, u8, IndexHash)> = HashSet::new();
            for package in ironworks.resources() {
                let entries = package.entries()?;
                log::info!("Enumerated {} installed files", entries.len());
                for entry in entries {
                    installed.insert((entry.repository, entry.category, entry.hash));
                }
            }

            let mut named = HashSet::with_capacity(installed.len());
            let mut present = Vec::with_capacity(master.path_count());
            let mut full = String::new();
            for (dir, names) in master.iter() {
                for name in names {
                    full.clear();
                    if !dir.is_empty() {
                        full.push_str(dir);
                        full.push('/');
                    }
                    full.push_str(name);
                    // Packages hash lowercased paths, and the list is not uniformly lowercase.
                    full.make_ascii_lowercase();

                    let Ok((repository, category)) = ironworks
                        .resources()
                        .first()
                        .context("no sqpack resource")?
                        .locate(&full)
                    else {
                        present.push(false);
                        continue;
                    };
                    let (split, whole) = IndexHash::of(&full);
                    let keys = split
                        .map(|hash| (repository, category, hash))
                        .into_iter()
                        .chain(std::iter::once((repository, category, whole)));
                    let mut found = false;
                    for key in keys {
                        if installed.contains(&key) {
                            named.insert(key);
                            found = true;
                        }
                    }
                    present.push(found);
                }
            }

            let mut unnamed: Vec<pathlist::Unnamed> = installed
                .iter()
                .filter(|key| !named.contains(*key))
                .map(|(repository, category, hash)| pathlist::Unnamed {
                    repository: *repository,
                    category: *category,
                    hash: match hash {
                        IndexHash::Split(hash) => *hash,
                        IndexHash::Whole(hash) => u64::from(*hash),
                    },
                    split: matches!(hash, IndexHash::Split(_)),
                })
                .collect();
            unnamed.sort_unstable_by_key(|file| {
                (file.repository, file.category, file.split, file.hash)
            });

            log::info!(
                "{slug}/{version}: {} of {} listed paths present, {} installed files unnamed",
                present.iter().filter(|p| **p).count(),
                present.len(),
                unnamed.len()
            );

            anyhow::Ok(pathlist::compress(&pathlist::encode_presence(
                &present, &unnamed, list_id,
            ))?)
        })?;

        let map = Bytes::from(map);
        self.write_stored(&key, &map).await;
        self.presence.insert(key, map.clone());
        Ok(map)
    }

    fn stored_path(&self, (list_id, slug, version): &(u64, Slug, GameVersion)) -> Option<PathBuf> {
        let format = pathlist::PRESENCE_VERSION;
        self.config
            .cache_directory
            .as_ref()
            .map(|dir| dir.join(format!("{list_id:016x}-{slug}-{version}.pdb{format}")))
    }

    async fn read_stored(&self, key: &(u64, Slug, GameVersion)) -> Option<Bytes> {
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

    async fn write_stored(&self, key: &(u64, Slug, GameVersion), map: &Bytes) {
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

    #[test]
    fn list_id_distinguishes_a_shifted_split() {
        let a = MasterList::parse("dir/ab\ndir/c\n");
        let b = MasterList::parse("dir/a\ndir/bc\n");
        assert_eq!(a.path_count(), b.path_count());
        assert_ne!(a.id(), b.id());
    }

    #[test]
    fn list_id_ignores_input_order() {
        let a = MasterList::parse("b/two\na/one\n");
        let b = MasterList::parse("a/one\nb/two\n");
        assert_eq!(a.id(), b.id());
    }
}
