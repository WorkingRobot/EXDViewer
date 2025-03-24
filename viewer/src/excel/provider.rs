use std::io::Cursor;

use anyhow::Result;
use binrw::{BinRead, BinResult, binread, helpers::until_exclusive, meta::ReadEndian};
use ironworks::{
    excel::Language,
    file::exh::{ColumnDefinition, PageDefinition},
    sestring::SeString,
};
use num_traits::FromBytes;

pub trait ExcelProvider {
    type Header: ExcelHeader;
    type Sheet: ExcelSheet;

    fn get_names(&self) -> &Vec<String>;
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

    fn get_row_id_at(&self, index: u32) -> Result<u32>;

    fn get_row_subrow_count(&self, row_id: u32) -> Result<u16>;

    fn get_row(&self, row_id: u32) -> Result<ExcelRow<'_>> {
        self.get_subrow(row_id, 0)
    }

    fn get_subrow(&self, row_id: u32, subrow_id: u16) -> Result<ExcelRow<'_>>;
}

#[derive(Debug, Clone, Copy)]
pub struct ExcelRow<'a> {
    data: &'a [u8],
    row_size: u16,
}

#[binread]
#[br(big)]
struct SeStringWrapper(#[br(parse_with = until_exclusive(|&byte| byte==0))] Vec<u8>);

impl<'a> ExcelRow<'a> {
    pub fn new(data: &'a [u8], row_size: u16) -> Self {
        Self { data, row_size }
    }

    pub fn row_size(&self) -> u16 {
        self.row_size
    }

    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    pub fn read_string(&self, offset: u32) -> anyhow::Result<SeString<'_>> {
        let offset = self.read::<u32>(offset)? + self.row_size() as u32;
        Ok(SeString::new(self.read_bw::<SeStringWrapper>(offset)?.0))
    }

    pub fn read_bool(&self, offset: u32) -> bool {
        self.data()[offset as usize] != 0
    }

    pub fn read_packed_bool(&self, offset: u32, bit: u8) -> bool {
        self.data()[offset as usize] & (1 << bit) != 0
    }

    pub fn read_bw<T: BinRead + ReadEndian>(&self, offset: u32) -> BinResult<T>
    where
        for<'b> <T as BinRead>::Args<'b>: Default,
    {
        T::read(&mut Cursor::new(&self.data()[offset as usize..]))
    }

    pub fn read<T: FromBytes>(
        &self,
        offset: u32,
    ) -> Result<T, <T::Bytes as TryFrom<&'a [u8]>>::Error>
    where
        T::Bytes: Sized + TryFrom<&'a [u8]>,
    {
        let size = std::mem::size_of::<T::Bytes>();
        // Check that the slice has enough bytes at the given offset.
        let slice: &T::Bytes =
            &self.data()[offset as usize..(offset as usize + size)].try_into()?;
        Ok(T::from_be_bytes(slice))
    }
}
