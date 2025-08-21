use std::io::{self, BufReader, Read, Seek, SeekFrom};

pub struct SmartBufReader<R: Read + Seek> {
    inner: BufReader<R>,
    last_pos: u64,
}

impl<R: Read + Seek> SmartBufReader<R> {
    #[allow(dead_code)]
    pub fn new(mut inner: R, capacity: usize) -> io::Result<Self> {
        Ok(Self {
            last_pos: inner.stream_position()?,
            inner: BufReader::with_capacity(capacity, inner),
        })
    }

    pub fn unchecked_new(inner: R, capacity: usize) -> Self {
        Self {
            last_pos: 0,
            inner: BufReader::with_capacity(capacity, inner),
        }
    }
}

impl<R: Read + Seek> Read for SmartBufReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let ret = self.inner.read(buf)?;
        if ret > 0 {
            self.last_pos += ret as u64;
        }
        Ok(ret)
    }
}

impl<R: Read + Seek> Seek for SmartBufReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let current_pos = self.last_pos;
        let relative_pos = match pos {
            SeekFrom::Start(pos) => {
                if pos == current_pos {
                    return Ok(current_pos);
                }
                match pos
                    .try_into()
                    .and_then(|p: i64| current_pos.try_into().map(|c: i64| p - c))
                {
                    Ok(offset) => offset,
                    Err(_) => {
                        log::error!("Seek position overflow: {} from {}", pos, current_pos);
                        let ret = self.inner.seek(SeekFrom::Start(pos))?;
                        self.last_pos = ret;
                        return Ok(ret);
                    }
                }
            }
            SeekFrom::End(offset) => {
                let ret = self.inner.seek(SeekFrom::End(offset))?;
                self.last_pos = ret;
                return Ok(ret);
            }
            SeekFrom::Current(offset) => offset,
        };
        let ret = self
            .inner
            .seek_relative(relative_pos)
            .map(|_| current_pos + relative_pos as u64)?;
        self.last_pos = ret;
        Ok(ret)
    }
}
