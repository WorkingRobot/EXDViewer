use std::io::{Read, Seek};

use binrw::{BinRead, BinResult, Endian, binread};
use ironworks::file::File;

#[binread]
#[derive(Debug)]
#[br(big, magic = b"EXDF")]
pub struct ExcelData {
    _version: u16,
    // unknown1: u16,
    #[br(pad_before = 2, temp)]
    index_size: u32,

    // unknown2: [u16; 10],
    /// Vector of rows contained within this page.
    #[br(
		pad_before = 20,
		count = index_size / 8,
	)]
    pub rows: Vec<RowDefinition>,

    #[br(parse_with = until_eof)]
    pub data: Vec<u8>,
}

impl File for ExcelData {
    fn read(mut stream: impl ironworks::FileStream) -> Result<Self, ironworks::Error> {
        Ok(<Self as BinRead>::read(&mut stream)?)
    }
}

/// Metadata of a row contained in a page.
#[binread]
#[derive(Debug)]
#[br(big)]
pub struct RowDefinition {
    /// Primary key ID of this row.
    pub id: u32,
    pub offset: u32,
}

impl RowDefinition {
    pub const SIZE: u32 = 8;
}

#[binread]
#[derive(Debug)]
#[br(big)]
pub struct RowHeader {
    pub data_size: u32,
    pub row_count: u16,
}

impl RowHeader {
    pub const SIZE: u32 = 6;
}

#[binread]
#[derive(Debug)]
#[br(big)]
pub struct SubrowHeader {
    pub id: u16,
}

impl SubrowHeader {
    pub const SIZE: u32 = 2;
}

fn until_eof<R: Read + Seek>(reader: &mut R, _: Endian, _: ()) -> BinResult<Vec<u8>> {
    let mut v = Vec::new();
    reader.read_to_end(&mut v)?;
    Ok(v)
}
