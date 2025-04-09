use std::io::{Read, Seek};

use web_sys::{File, FileReaderSync, js_sys::Uint8Array};

use super::map_jserr;

pub struct SyncAccessFile {
    handle: File,
    reader: FileReaderSync,
    offset: u64,
}

impl SyncAccessFile {
    pub fn new(handle: File) -> std::io::Result<Self> {
        Ok(Self {
            handle,
            reader: FileReaderSync::new().map_err(map_jserr)?,
            offset: 0,
        })
    }

    fn into_u64(value: f64) -> std::io::Result<u64> {
        if value.is_nan()
            || value.fract() != 0.0
            || value < u64::MIN as f64
            || value > u64::MAX as f64
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("f64 {value:?} is not convertible to u64"),
            ));
        }
        return Ok(value.trunc() as u64);
    }

    fn into_f64(value: u64) -> std::io::Result<f64> {
        if u64::BITS - value.leading_zeros() >= f64::MANTISSA_DIGITS {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("u64 {value:?} is not convertible to f64"),
            ));
        }
        return Ok(value as f64);
    }

    fn get_size(&self) -> std::io::Result<u64> {
        Self::into_u64(self.handle.size())
    }
}

impl Read for SyncAccessFile {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let start = Self::into_f64(self.offset)?;
        let end = Self::into_f64(self.offset + buf.len() as u64)?.min(self.handle.size());
        let blob = self
            .handle
            .slice_with_f64_and_f64(start, end)
            .map_err(map_jserr)?;
        let buffer = self.reader.read_as_array_buffer(&blob).map_err(map_jserr)?;
        let array = Uint8Array::new(&buffer);
        array.copy_to(
            buf.get_mut(..(array.length() as usize))
                .ok_or(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("buffer is too small"),
                ))?,
        );
        self.offset += array.length() as u64;
        Ok(array.length() as usize)
    }
}

impl Seek for SyncAccessFile {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        match pos {
            std::io::SeekFrom::Start(v) => {
                self.offset = v;
            }
            std::io::SeekFrom::End(v) => {
                self.offset = self.get_size()?.checked_add_signed(v).ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "offset would over/underflow",
                    )
                })?;
            }
            std::io::SeekFrom::Current(v) => {
                self.offset.checked_add_signed(v).ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "offset would over/underflow",
                    )
                })?;
            }
        };
        Ok(self.offset)
    }
}
