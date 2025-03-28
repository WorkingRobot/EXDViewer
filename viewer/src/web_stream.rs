use std::{
    io::{Read, Seek},
    ops::ControlFlow,
    sync::mpsc::{Receiver, RecvError, Sender},
};

use ehttp::{Request, streaming::Part};

pub struct WebStream {
    data: Vec<u8>,
    pos: usize,
    #[cfg(not(target_arch = "wasm32"))]
    rx: Receiver<Vec<u8>>,
}

impl WebStream {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(request: Request, only_ok: bool) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        ehttp::streaming::fetch(request, move |part| Self::on_recv(part, only_ok, &tx));
        Self {
            data: Vec::new(),
            pos: 0,
            rx,
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn new(request: Request, only_ok: bool) -> Self {
        let resp: Result<ehttp::Response, String>;
        {
            let (tx, rx) = std::sync::mpsc::channel();
            ehttp::fetch(request, move |resp| {
                let _ = tx.send(resp);
            });
            resp = rx.recv().unwrap();
        }

        let data = match resp {
            Ok(resp) if resp.ok || !only_ok => resp.bytes,
            _ => Vec::new(),
        };
        Self { data, pos: 0 }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn on_recv(
        part: ehttp::Result<Part>,
        cancel_on_err: bool,
        tx: &Sender<Vec<u8>>,
    ) -> ControlFlow<()> {
        match part {
            Ok(part) => {
                match part {
                    Part::Response(response) => {
                        if cancel_on_err && !response.ok {
                            return ControlFlow::Break(());
                        }
                    }
                    Part::Chunk(chunk) => {
                        if tx.send(chunk).is_err() {
                            return ControlFlow::Break(());
                        }
                    }
                }
                ControlFlow::Continue(())
            }
            Err(_) => ControlFlow::Break(()),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn try_process_recv(&mut self) {
        while let Ok(chunk) = self.rx.try_recv() {
            self.data.extend(chunk);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn process_recv(&mut self, additional_count: usize) -> usize {
        let mut copied = 0;
        while copied < additional_count {
            let data = match self.rx.recv() {
                Ok(data) => data,
                Err(RecvError) => break,
            };
            copied += data.len();
            self.data.extend(data);
        }
        copied
    }
}

impl Read for WebStream {
    fn read(&mut self, mut buf: &mut [u8]) -> std::io::Result<usize> {
        #[cfg(not(target_arch = "wasm32"))]
        self.try_process_recv();
        let avail_data = &self.data[self.pos..];
        let copy_len = avail_data.len().min(buf.len());
        buf[..copy_len].copy_from_slice(&avail_data[..copy_len]);
        buf = &mut buf[copy_len..];
        self.pos += copy_len;

        #[cfg(not(target_arch = "wasm32"))]
        if !buf.is_empty() {
            let more_data = self.process_recv(buf.len());
            let avail_data = &self.data[self.pos..self.pos + more_data];
            let old_copy_len = copy_len;
            let copy_len = avail_data.len().min(buf.len());
            buf[..copy_len].copy_from_slice(&avail_data[..copy_len]);
            self.pos += copy_len;
            return Ok(old_copy_len + copy_len);
        }

        Ok(copy_len)
    }
}

impl Seek for WebStream {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        match pos {
            std::io::SeekFrom::Start(pos) => {
                self.pos = pos as usize;
            }
            std::io::SeekFrom::End(pos) => {
                self.pos = self.data.len() - pos as usize;
            }
            std::io::SeekFrom::Current(pos) => {
                self.pos += pos as usize;
            }
        }
        Ok(self.pos as u64)
    }
}
