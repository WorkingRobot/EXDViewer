use anyhow::Result;
use async_trait::async_trait;
use binrw::{BinRead, BinResult, meta::ReadEndian};
use futures_util::{FutureExt, future::LocalBoxFuture};
use intmap::IntMap;
use ironworks::{
    excel::{Language, path},
    file::{
        File,
        exd::{ExcelData, RowHeader, SubrowHeader},
        exh::{ColumnDefinition, PageDefinition, SheetKind},
    },
};
use std::{cell::RefCell, io::Cursor, num::NonZeroUsize, ops::Range, sync::Arc};

use crate::value_cache::KeyedCache;

use super::provider::{ExcelHeader, ExcelProvider, ExcelRow, ExcelSheet};

#[async_trait(?Send)]
pub trait FileProvider {
    async fn file<T: File>(&self, path: &str) -> Result<T, ironworks::Error>;
}

#[async_trait(?Send)]
pub trait ExcelFileProvider {
    async fn list(&self) -> Result<ironworks::file::exl::ExcelList, ironworks::Error>;

    async fn header(
        &self,
        name: &str,
    ) -> Result<ironworks::file::exh::ExcelHeader, ironworks::Error>;

    async fn data(
        &self,
        name: &str,
        start_id: u32,
        language: Language,
    ) -> Result<ExcelData, ironworks::Error>;
}

#[async_trait(?Send)]
impl<T: FileProvider> ExcelFileProvider for T {
    async fn list(&self) -> Result<ironworks::file::exl::ExcelList, ironworks::Error> {
        self.file(path::exl()).await
    }

    async fn header(
        &self,
        name: &str,
    ) -> Result<ironworks::file::exh::ExcelHeader, ironworks::Error> {
        self.file(&path::exh(name)).await
    }

    async fn data(
        &self,
        name: &str,
        start_id: u32,
        language: Language,
    ) -> Result<ExcelData, ironworks::Error> {
        self.file(&path::exd(name, start_id, language)).await
    }
}

#[async_trait(?Send)]
impl ExcelFileProvider for Box<dyn ExcelFileProvider> {
    async fn list(&self) -> Result<ironworks::file::exl::ExcelList, ironworks::Error> {
        self.as_ref().list().await
    }

    async fn header(
        &self,
        name: &str,
    ) -> Result<ironworks::file::exh::ExcelHeader, ironworks::Error> {
        self.as_ref().header(name).await
    }

    async fn data(
        &self,
        name: &str,
        start_id: u32,
        language: Language,
    ) -> Result<ExcelData, ironworks::Error> {
        self.as_ref().data(name, start_id, language).await
    }
}

pub struct CachedProvider<T: ExcelFileProvider>(Arc<CachedProviderImpl<T>>);

impl<T: ExcelFileProvider> Clone for CachedProvider<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

struct CachedProviderImpl<T: ExcelFileProvider> {
    provider: T,
    names: Vec<String>,
    cache: RefCell<lru::LruCache<String, Arc<CacheEntry>>>,
}

struct CacheEntry {
    pub header: BaseHeader,
    pub cache: RefCell<KeyedCache<Language, BaseSheet>>,
}

impl<T: ExcelFileProvider> CachedProvider<T> {
    pub async fn new(provider: T, size: NonZeroUsize) -> Result<Self, ironworks::Error> {
        Ok(Self(Arc::new(CachedProviderImpl {
            names: provider
                .list()
                .await?
                .iter()
                .map(|s| s.into_owned())
                .collect(),
            provider,
            cache: RefCell::new(lru::LruCache::new(size)),
        })))
    }

    async fn use_entry<'a, R>(
        &'a self,
        name: &str,
        op: impl FnOnce(Arc<CacheEntry>) -> LocalBoxFuture<'a, R>,
    ) -> Result<R, ironworks::Error> {
        let mut cache = self.0.cache.borrow_mut();

        let ret = match cache.get(name) {
            Some(ret) => ret,
            None => {
                let header = self.0.provider.header(name).await?;
                cache.get_or_insert_ref(name, || {
                    Arc::new(CacheEntry {
                        header: BaseHeader::new(name.to_string(), header),
                        cache: RefCell::new(KeyedCache::new()),
                    })
                })
            }
        };
        Ok(op(ret.clone()).await)
    }
}

impl<T: ExcelFileProvider> ExcelProvider for CachedProvider<T> {
    type Header = BaseHeader;

    type Sheet = BaseSheet;

    fn get_names(&self) -> &Vec<String> {
        &self.0.names
    }

    async fn get_header(&self, name: &str) -> Result<BaseHeader> {
        Ok(self
            .use_entry(name, |a| async move { a.header.clone() }.boxed_local())
            .await?)
    }

    async fn get_sheet(&self, name: &str, language: Language) -> Result<BaseSheet> {
        self.use_entry(name, |a| {
            async move {
                a.cache
                    .borrow_mut()
                    .try_get_or_set_ref_async(&language, || {
                        let language = if a.header.languages().contains(&language) {
                            language
                        } else {
                            Language::None
                        };
                        BaseSheet::new(a.header.clone(), language, &self.0.provider)
                    })
                    .await
                    .cloned()
            }
            .boxed_local()
        })
        .await?
    }
}

#[derive(Debug, Clone)]
pub struct BaseHeader {
    imp: Arc<BaseHeaderImpl>,
}

#[derive(Debug)]
struct BaseHeaderImpl {
    pub name: String,
    pub header: ironworks::file::exh::ExcelHeader,
    pub languages: Vec<Language>,
}

impl BaseHeader {
    pub fn new(name: String, header: ironworks::file::exh::ExcelHeader) -> Self {
        let languages = header
            .languages()
            .iter()
            .map(|l| Language::try_from(*l).unwrap())
            .collect();
        Self {
            imp: Arc::new(BaseHeaderImpl {
                name,
                header,
                languages,
            }),
        }
    }
}

impl ExcelHeader for BaseHeader {
    fn name(&self) -> &str {
        &self.imp.name
    }

    fn columns(&self) -> &Vec<ColumnDefinition> {
        self.imp.header.columns()
    }

    fn row_intervals(&self) -> &Vec<PageDefinition> {
        self.imp.header.pages()
    }

    fn languages(&self) -> &Vec<Language> {
        &self.imp.languages
    }

    fn has_subrows(&self) -> bool {
        self.imp.header.kind() == SheetKind::Subrows
    }
}

#[derive(Debug, Clone)]
pub struct BaseSheet {
    imp: Arc<BaseSheetImpl>,
}

#[derive(Debug)]
struct BaseSheetImpl {
    header: BaseHeader,
    pages: Vec<Page>,
    subrow_count: u32,
    row_lookup: IntMap<u32, RowLocation>,
    row_id_lookup: Vec<(u32, Range<u32>)>,
}

impl BaseSheet {
    pub async fn new(
        header: BaseHeader,
        language: Language,
        provider: &impl ExcelFileProvider,
    ) -> Result<Self> {
        if !header.languages().contains(&language) {
            return Err(anyhow::anyhow!(
                "Language {:?} not found in sheet {}",
                language,
                header.name()
            ));
        }

        let has_subrows = header.has_subrows();
        let row_size = header.imp.header.row_size();
        let row_count = header
            .imp
            .header
            .pages()
            .iter()
            .fold(0, |acc, p| acc + p.row_count());
        let mut row_lookup = IntMap::with_capacity(row_count as usize);
        let mut pages = Vec::with_capacity(header.imp.header.pages().len());
        let mut row_id_lookup = Vec::with_capacity(header.imp.header.pages().len());
        let mut current_row_range: Option<(u32, Range<u32>)> = None;
        for page_def in header.imp.header.pages() {
            let data = provider
                .data(&header.imp.name, page_def.start_id(), language)
                .await?;
            let page = Page {
                row_size,
                offset: data.data_offset.try_into()?,
                data: data.data,
            };
            let page_idx = pages.len() as u16;
            for row_def in data.rows {
                let header = page.read_bw::<RowHeader>(row_def.offset)?;
                if !has_subrows {
                    debug_assert_eq!(header.row_count, 1);
                }
                let subrow_count = if has_subrows { header.row_count } else { 1 };
                let location = RowLocation {
                    offset: row_def.offset,
                    page_idx,
                    subrow_count,
                };

                match &mut current_row_range {
                    Some(range) if range.1.end == row_def.id => range.1.end += 1,
                    Some(range) => {
                        row_id_lookup.push(range.clone());
                        current_row_range =
                            Some((row_lookup.len() as u32, row_def.id..row_def.id + 1));
                    }
                    None => {
                        current_row_range =
                            Some((row_lookup.len() as u32, row_def.id..row_def.id + 1))
                    }
                }
                row_lookup.insert(row_def.id, location);
            }
            pages.push(page);
        }

        if let Some(range) = current_row_range {
            row_id_lookup.push(range);
        }

        let subrow_count: u32 = row_lookup.values().map(|l| l.subrow_count as u32).sum();

        Ok(Self {
            imp: Arc::new(BaseSheetImpl {
                header,
                pages,
                subrow_count,
                row_lookup,
                row_id_lookup,
            }),
        })
    }
}

impl ExcelHeader for BaseSheet {
    fn name(&self) -> &str {
        self.imp.header.name()
    }

    fn columns(&self) -> &Vec<ColumnDefinition> {
        self.imp.header.columns()
    }

    fn row_intervals(&self) -> &Vec<PageDefinition> {
        self.imp.header.row_intervals()
    }

    fn languages(&self) -> &Vec<Language> {
        self.imp.header.languages()
    }

    fn has_subrows(&self) -> bool {
        self.imp.header.has_subrows()
    }
}

impl ExcelSheet for BaseSheet {
    fn row_count(&self) -> u32 {
        self.imp.row_lookup.len() as u32
    }

    fn subrow_count(&self) -> u32 {
        self.imp.subrow_count
    }

    fn get_row_id_at(&self, index: u32) -> Result<u32> {
        if index >= self.row_count() {
            return Err(anyhow::anyhow!(
                "Row index {} out of bounds for sheet {}",
                index,
                self.name()
            ));
        }
        let range_idx = self
            .imp
            .row_id_lookup
            .binary_search_by_key(&index, |pair| pair.0)
            .unwrap_or_else(|i| i - 1);
        let (start_idx, id_range) = self.imp.row_id_lookup.get(range_idx).ok_or_else(|| {
            anyhow::anyhow!(
                "Range index {} out of bounds for sheet {}",
                range_idx,
                self.name()
            )
        })?;
        if !(*start_idx..start_idx + (id_range.end - id_range.start)).contains(&index) {
            return Err(anyhow::anyhow!(
                "Row index {} out of bounds for range {}..{} in sheet {}",
                index,
                id_range.start,
                id_range.end,
                self.name()
            ));
        }
        Ok(id_range.start + (index - *start_idx))
    }

    fn get_row_subrow_count(&self, row_id: u32) -> Result<u16> {
        Ok(self
            .imp
            .row_lookup
            .get(row_id)
            .ok_or_else(|| anyhow::anyhow!("Row ID {} not found in sheet {}", row_id, self.name()))?
            .subrow_count)
    }

    fn get_subrow(&self, row_id: u32, subrow_id: u16) -> Result<ExcelRow<'_>> {
        let location = self.imp.row_lookup.get(row_id).ok_or_else(|| {
            anyhow::anyhow!("Row ID {} not found in sheet {}", row_id, self.name())
        })?;
        if location.subrow_count <= subrow_id {
            return Err(anyhow::anyhow!(
                "Subrow ID {} out of bounds for row {} in sheet {}",
                subrow_id,
                row_id,
                self.name()
            ));
        }
        let page = &self.imp.pages[location.page_idx as usize];
        let (offset, length) = if self.has_subrows() {
            (
                location.offset
                    + RowHeader::SIZE as u32
                    + subrow_id as u32 * (SubrowHeader::SIZE as u32 + page.row_size as u32)
                    + SubrowHeader::SIZE as u32,
                page.row_size as u32,
            )
        } else {
            (
                location.offset + RowHeader::SIZE as u32,
                page.read_bw::<RowHeader>(location.offset)?.data_size,
            )
        };
        page.get_row(offset, length)
    }
}

#[derive(Debug)]
struct Page {
    row_size: u16,
    offset: u32,
    data: Vec<u8>,
}

impl Page {
    pub fn get_row(&self, offset: u32, length: u32) -> Result<ExcelRow<'_>> {
        Ok(ExcelRow::new(
            self.data
                .get((offset - self.offset) as usize..(offset - self.offset + length) as usize)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Failed to get page data for offset {offset} length {length} ({})",
                        self.data.len() + self.offset as usize
                    )
                })?,
            self.row_size,
        ))
    }

    pub fn read_bw<T: BinRead + ReadEndian>(&self, offset: u32) -> BinResult<T>
    where
        for<'a> <T as BinRead>::Args<'a>: Default,
    {
        T::read(&mut Cursor::new(
            &self.data[offset as usize - self.offset as usize..],
        ))
    }
}

#[derive(Debug)]
struct RowLocation {
    pub offset: u32,
    pub page_idx: u16,
    pub subrow_count: u16,
}
