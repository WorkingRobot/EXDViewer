use std::io::{self, SeekFrom};
use std::sync::mpsc;
use std::thread;

use tokio::io::{AsyncRead, AsyncSeek};
use tokio::runtime::{Builder, Runtime};

enum Cmd {
    Read {
        // size requested
        len: usize,
        resp: mpsc::Sender<io::Result<Vec<u8>>>,
    },
    Seek {
        pos: SeekFrom,
        resp: mpsc::Sender<io::Result<u64>>,
    },
    Close,
}

pub struct BlockingReader<R: AsyncRead + AsyncSeek + Unpin + Send + 'static> {
    tx: mpsc::Sender<Cmd>,
    // Join handle to ensure thread exits before drop finishes (optional; we ignore errors on drop)
    join: Option<thread::JoinHandle<()>>,
    // small read buffer to reduce round-trip overhead for many tiny reads
    buf: Vec<u8>,
    buf_pos: usize,
    buf_len: usize,
    _marker: std::marker::PhantomData<R>,
}

impl<R: AsyncRead + AsyncSeek + Unpin + Send + 'static> BlockingReader<R> {
    pub fn new(stream: R) -> Self {
        let (tx, rx) = mpsc::channel::<Cmd>();

        // Dedicated thread owning its own runtime and the async stream.
        let join = thread::spawn(move || {
            // Separate runtime (multi-thread not required, keep it lightweight current_thread).
            let rt = Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("rt build");
            worker_loop(rt, stream, rx);
        });

        Self {
            tx,
            join: Some(join),
            buf: Vec::with_capacity(64 * 1024), // 64KiB buffer
            buf_pos: 0,
            buf_len: 0,
            _marker: std::marker::PhantomData,
        }
    }

    fn refill_buffer(&mut self) -> io::Result<()> {
        self.buf_pos = 0;
        self.buf_len = 0;
        // request a fresh fill of up to capacity
        let cap = self.buf.capacity();
        let (rtx, rrx) = mpsc::channel();
        self.tx
            .send(Cmd::Read {
                len: cap,
                resp: rtx,
            })
            .map_err(|_| io::Error::other("worker dropped"))?;
        let chunk = rrx
            .recv()
            .map_err(|_| io::Error::other("worker dropped"))??;
        if self.buf.capacity() < chunk.len() {
            self.buf = chunk; // unexpected but handle gracefully
        } else {
            unsafe {
                self.buf.set_len(chunk.len());
            }
            self.buf.copy_from_slice(&chunk);
        }
        self.buf_len = self.buf.len();
        Ok(())
    }
}

fn worker_loop<R: AsyncRead + AsyncSeek + Unpin + Send + 'static>(
    rt: Runtime,
    mut stream: R,
    rx: mpsc::Receiver<Cmd>,
) {
    while let Ok(cmd) = rx.recv() {
        match cmd {
            Cmd::Read { len, resp } => {
                let res = rt.block_on(async {
                    use tokio::io::AsyncReadExt;
                    let mut buf = vec![0u8; len];
                    let n = stream.read(&mut buf).await?; // single read (may read < len)
                    buf.truncate(n);
                    Ok::<_, io::Error>(buf)
                });
                let _ = resp.send(res);
            }
            Cmd::Seek { pos, resp } => {
                let res = rt.block_on(async {
                    use tokio::io::AsyncSeekExt;
                    stream.seek(pos).await
                });
                let _ = resp.send(res);
            }
            Cmd::Close => break,
        }
    }
}

impl<R: AsyncRead + AsyncSeek + Unpin + Send + 'static> io::Read for BlockingReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        // Serve from buffer if possible
        if self.buf_pos >= self.buf_len {
            self.refill_buffer()?;
            if self.buf_len == 0 {
                return Ok(0);
            }
        }
        let available = &self.buf[self.buf_pos..self.buf_len];
        let n = available.len().min(out.len());
        out[..n].copy_from_slice(&available[..n]);
        self.buf_pos += n;
        Ok(n)
    }
}

impl<R: AsyncRead + AsyncSeek + Unpin + Send + 'static> io::Seek for BlockingReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        // invalidate buffer on any seek
        self.buf_pos = 0;
        self.buf_len = 0;
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(Cmd::Seek { pos, resp: tx })
            .map_err(|_| io::Error::other("worker dropped"))?;
        rx.recv().map_err(|_| io::Error::other("worker dropped"))?
    }
}

impl<R: AsyncRead + AsyncSeek + Unpin + Send + 'static> Drop for BlockingReader<R> {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Close);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}
