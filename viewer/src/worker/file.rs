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

    fn read_for(&mut self, len: u64) -> std::io::Result<Uint8Array> {
        let start = Self::into_f64(self.offset)?;
        let end = Self::into_f64(self.offset + len)?.min(self.handle.size());
        let blob = self
            .handle
            .slice_with_f64_and_f64(start, end)
            .map_err(map_jserr)?;
        let buffer = self.reader.read_as_array_buffer(&blob).map_err(map_jserr)?;
        let array = Uint8Array::new(&buffer);
        self.offset += array.length() as u64;
        Ok(array)
    }
}

impl Read for SyncAccessFile {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.read_for(buf.len() as u64).and_then(|array| {
            array.copy_to(
                buf.get_mut(..(array.length() as usize))
                    .ok_or(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("buffer is too small"),
                    ))?,
            );
            Ok(array.length() as usize)
        })
    }

    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> std::io::Result<usize> {
        let size = self.get_size()?;
        let offset = self.offset;
        if offset >= size {
            return Ok(0);
        }
        let more = size - offset;
        buf.reserve(more as usize);

        self.read_for(more).and_then(|array| {
            let data = buf
                .spare_capacity_mut()
                .get_mut(..(array.length() as usize))
                .ok_or(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("buffer is too small"),
                ))?;
            array.copy_to_uninit(data);

            // SAFETY:
            // 1. `spare_capacity_mut` is never past the capacity.
            // 2. `copy_to_uninit` guarantees that the buffer is initialized;
            //    it panics if `data.len() != array.length()` anyways.
            unsafe {
                buf.set_len(buf.len() + array.length() as usize);
            }
            Ok(array.length() as usize)
        })
    }

    fn read_to_string(&mut self, buf: &mut String) -> std::io::Result<usize> {
        let mut bytes = Vec::new();
        let size = self.read_to_end(&mut bytes)?;
        buf.push_str(std::str::from_utf8(&bytes).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "could not convert bytes to string",
            )
        })?);
        Ok(size)
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
