use std::{
    future::poll_fn,
    io::{self},
    pin::Pin,
};

use tokio::{
    io::{AsyncRead, AsyncSeek, ReadBuf},
    runtime::Handle,
};

pub struct BlockingReader<R: AsyncRead + AsyncSeek + Unpin + Send + 'static> {
    stream: R,
}

impl<R: AsyncRead + AsyncSeek + Unpin + Send + 'static> BlockingReader<R> {
    pub fn new(stream: R) -> Self {
        Self { stream }
    }
}

impl<R: AsyncRead + AsyncSeek + Unpin + Send + 'static> io::Read for BlockingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        log::trace!(
            "BlockingReader::read called with buffer of size {}",
            buf.len()
        );
        let ret = tokio::task::block_in_place(|| {
            Handle::current().block_on(async {
                let mut read_buf = ReadBuf::new(buf);
                let initial_filled = read_buf.filled().len();
                poll_fn(|cx| Pin::new(&mut self.stream).poll_read(cx, &mut read_buf)).await?;
                Ok(read_buf.filled().len() - initial_filled)
            })
        });
        log::trace!(
            "BlockingReader::read({}) returning {ret:?} bytes",
            buf.len()
        );
        ret
    }
}
impl<R: AsyncRead + AsyncSeek + Unpin + Send + 'static> io::Seek for BlockingReader<R> {
    fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
        log::trace!("BlockingReader::seek called with position {:?}", pos);
        let seek_result = tokio::task::block_in_place(|| {
            Handle::current().block_on(async move {
                Pin::new(&mut self.stream).start_seek(pos)?;
                poll_fn(|cx| Pin::new(&mut self.stream).poll_complete(cx)).await
            })
        });
        log::trace!("BlockingReader::seek returning position {seek_result:?}");
        seek_result
    }
}
