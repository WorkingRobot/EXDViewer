use std::{collections::HashMap, error::Error, io::Cursor};

use anyhow::Result;
use async_trait::async_trait;
use binrw::{BinRead, binread, helpers::until_exclusive, meta::ReadEndian};
use either::Either;
use image::RgbaImage;
use ironworks::{
    excel::Language,
    file::exh::{ColumnDefinition, PageDefinition},
    sestring::SeStr,
};
use num_traits::FromBytes;
use url::Url;

#[async_trait(?Send)]
pub trait ExcelProvider {
    type Header: ExcelHeader;
    type Sheet: ExcelSheet;

    fn get_entries(&self) -> &HashMap<String, i32>;
    async fn get_icon(&self, icon_id: u32, hires: bool) -> Result<Either<Url, RgbaImage>>;
    async fn get_sheet(&self, name: &str, language: Language) -> Result<Self::Sheet>;
    async fn get_header(&self, name: &str) -> Result<Self::Header>;
}

pub trait ExcelHeader {
    fn name(&self) -> &str;
    fn columns(&self) -> &Vec<ColumnDefinition>;
    fn row_intervals(&self) -> &Vec<PageDefinition>;
    fn languages(&self) -> &Vec<Language>;
    fn has_subrows(&self) -> bool;
}

pub trait ExcelSheet: ExcelHeader {
    fn row_count(&self) -> u32;
    fn subrow_count(&self) -> u32;

    fn get_row_ids(&self) -> impl Iterator<Item = u32>;

    fn get_subrow_ids(&self) -> impl Iterator<Item = (u32, u16)> {
        self.get_row_ids().flat_map(|row_id| {
            (0..self.get_row_subrow_count(row_id).unwrap())
                .map(move |subrow_id| (row_id, subrow_id))
        })
    }

    fn get_row_id_at(&self, index: u32) -> Result<u32>;

    fn get_row_subrow_count(&self, row_id: u32) -> Result<u16>;

    fn get_row(&self, row_id: u32) -> Result<ExcelRow<'_>> {
        self.get_subrow(row_id, 0)
    }

    fn get_subrow(&self, row_id: u32, subrow_id: u16) -> Result<ExcelRow<'_>>;
}

#[derive(Debug)]
pub struct ExcelPage {
    pub row_size: u16,
    pub data_offset: u32,
    // exd file: [data offset bytes] [data]
    pub data: Vec<u8>,
}

impl ExcelPage {
    fn get_range(&self, offset: u32, size: u32) -> anyhow::Result<&[u8]> {
        self.data
            .get((offset - self.data_offset) as usize..(offset - self.data_offset + size) as usize)
            .ok_or_else(|| anyhow::anyhow!("Couldn't seek to offset {offset} in row"))
    }

    fn get_slice(&self, offset: u32) -> anyhow::Result<&[u8]> {
        self.data
            .get((offset - self.data_offset) as usize..)
            .ok_or_else(|| anyhow::anyhow!("Couldn't seek to offset {offset} in row"))
    }

    fn get_cursor(&self, offset: u32) -> anyhow::Result<Cursor<&[u8]>> {
        let data = self.get_slice(offset)?;
        Ok(Cursor::new(data))
    }

    pub fn read_string(&self, offset: u32, string_offset: u32) -> anyhow::Result<&'_ SeStr> {
        let offset = string_offset + self.read::<u32>(offset)?;
        let data_slice = self.get_slice(offset)?;
        let data_len = data_slice
            .iter()
            .position(|p| *p == 0)
            .ok_or_else(|| anyhow::anyhow!("Couldn't find null terminator for string"))?;
        let string_slice = &data_slice[..data_len];
        Ok(string_slice.into())
    }

    pub fn read_bool(&self, offset: u32) -> anyhow::Result<bool> {
        Ok(self.get_range(offset, 1)?[0] != 0)
    }

    pub fn read_packed_bool(&self, offset: u32, bit: u8) -> anyhow::Result<bool> {
        Ok(self.get_range(offset, 1)?[0] & (1 << bit) != 0)
    }

    pub fn read_bw<T: BinRead + ReadEndian>(&self, offset: u32) -> anyhow::Result<T>
    where
        for<'b> <T as BinRead>::Args<'b>: Default,
    {
        Ok(T::read(&mut self.get_cursor(offset)?)?)
    }

    pub fn read<'a, T: FromBytes>(&'a self, offset: u32) -> anyhow::Result<T>
    where
        T::Bytes: Sized + TryFrom<&'a [u8]>,
        <<T as FromBytes>::Bytes as TryFrom<&'a [u8]>>::Error: Sync + Send + Error + 'static,
    {
        let size = std::mem::size_of::<T::Bytes>() as u32;
        // Check that the slice has enough bytes at the given offset.
        let slice: &T::Bytes = &self.get_range(offset, size)?.try_into()?;
        Ok(T::from_be_bytes(slice))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ExcelRow<'a> {
    page: &'a ExcelPage,
    offset: u32,
    string_offset: u32,
}

#[binread]
#[br(big)]
struct SeStringWrapper(#[br(parse_with = until_exclusive(|&byte| byte==0))] Vec<u8>);

impl<'a> ExcelRow<'a> {
    pub fn new(page: &'a ExcelPage, offset: u32, string_offset: u32) -> Self {
        Self {
            page,
            offset,
            string_offset,
        }
    }

    pub fn read_string(&self, offset: u32) -> anyhow::Result<&'_ SeStr> {
        self.page
            .read_string(self.offset + offset, self.string_offset)
    }

    pub fn read_bool(&self, offset: u32) -> anyhow::Result<bool> {
        self.page.read_bool(self.offset + offset)
    }

    pub fn read_packed_bool(&self, offset: u32, bit: u8) -> anyhow::Result<bool> {
        self.page.read_packed_bool(self.offset + offset, bit)
    }

    pub fn read<T: FromBytes>(&self, offset: u32) -> anyhow::Result<T>
    where
        T::Bytes: Sized + TryFrom<&'a [u8]>,
        <<T as FromBytes>::Bytes as TryFrom<&'a [u8]>>::Error: Sync + Send + Error + 'static,
    {
        self.page.read(self.offset + offset)
    }
}
