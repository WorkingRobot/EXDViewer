use std::{
    io::{self, SeekFrom},
    pin::Pin,
    slice,
    sync::{OnceLock, mpsc},
};

use futures_util::future::poll_fn;
use tokio::runtime::Handle;
use tokio::{
    io::{AsyncRead, AsyncSeek, ReadBuf},
    sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
};

static ASYNC_RT: OnceLock<Handle> = OnceLock::new();

pub fn init_runtime() {
    ASYNC_RT
        .set(Handle::current())
        .expect("Async runtime already initialized");
}

enum Request {
    ReadRaw {
        ptr: usize,
        len: usize,
        resp: mpsc::Sender<io::Result<usize>>,
    },
    Seek {
        pos: SeekFrom,
        resp: mpsc::Sender<io::Result<u64>>,
    },
    Close,
}

pub struct BlockingReader<R: AsyncRead + AsyncSeek + Unpin + Send + 'static> {
    tx: UnboundedSender<Request>,
    _marker: std::marker::PhantomData<R>,
}

impl<R: AsyncRead + AsyncSeek + Unpin + Send + 'static> BlockingReader<R> {
    pub fn new(mut stream: R) -> Self {
        let (tx, mut rx): (UnboundedSender<Request>, UnboundedReceiver<Request>) =
            unbounded_channel();

        ASYNC_RT.get().unwrap().spawn(async move {
            // worker loop; stream is owned here
            while let Some(req) = rx.recv().await {
                match req {
                    Request::ReadRaw { ptr, len, resp } => {
                        // SAFETY: we will create a &mut [u8] from ptr/len.
                        // The caller must guarantee `ptr` is valid for writes for the duration
                        // until we send the response (synchronous recv on caller side).
                        let res = unsafe {
                            // create a temporary slice referencing caller memory
                            let buf_slice = slice::from_raw_parts_mut(ptr as *mut u8, len);
                            let mut read_buf = ReadBuf::new(buf_slice);
                            // poll_read into the raw slice
                            let poll_res: io::Result<()> =
                                poll_fn(|cx| Pin::new(&mut stream).poll_read(cx, &mut read_buf))
                                    .await;
                            match poll_res {
                                Ok(()) => Ok(read_buf.filled().len()),
                                Err(e) => Err(e),
                            }
                        };
                        let _ = resp.send(res);
                    }

                    Request::Seek { pos, resp } => {
                        let res = async {
                            Pin::new(&mut stream).start_seek(pos)?;
                            poll_fn(|cx| Pin::new(&mut stream).poll_complete(cx)).await
                        }
                        .await;
                        let _ = resp.send(res);
                    }

                    Request::Close => break,
                }
            }
        });

        Self {
            tx,
            _marker: std::marker::PhantomData,
        }
    }
}

impl<R: AsyncRead + AsyncSeek + Unpin + Send + 'static> io::Read for BlockingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let (tx, rx) = mpsc::channel();
        // send raw pointer; caller must wait synchronously (so pointer stays valid)
        let ptr = buf.as_mut_ptr();
        let len = buf.len();
        self.tx
            .send(Request::ReadRaw {
                ptr: ptr as usize,
                len,
                resp: tx,
            })
            .map_err(|_| io::Error::other("worker dropped"))?;

        // block until worker writes into `buf` and replies with number of bytes written
        rx.recv().map_err(|_| io::Error::other("worker dropped"))?
    }
}

impl<R: AsyncRead + AsyncSeek + Unpin + Send + 'static> io::Seek for BlockingReader<R> {
    fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(Request::Seek { pos, resp: tx })
            .map_err(|_| io::Error::other("worker dropped"))?;
        rx.recv().map_err(|_| io::Error::other("worker dropped"))?
    }
}

impl<R: AsyncRead + AsyncSeek + Unpin + Send + 'static> Drop for BlockingReader<R> {
    fn drop(&mut self) {
        let _ = self.tx.send(Request::Close);
    }
}
