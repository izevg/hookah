//! A tiny HTTP/1.0-style server on loopback, enough for the update client: GET only, one
//! response per connection, a log of the paths requested.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// One canned response.
pub struct Route {
    path: String,
    status: u16,
    body: Vec<u8>,
}

impl Route {
    pub fn ok(path: &str, body: Vec<u8>) -> Self {
        Self::status(path, 200, body)
    }

    pub fn status(path: &str, status: u16, body: Vec<u8>) -> Self {
        Self {
            path: path.to_string(),
            status,
            body,
        }
    }
}

pub struct Server {
    addr: SocketAddr,
    log: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    pub fn start(routes: Vec<Route>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (log, stop) = (Arc::clone(&log), Arc::clone(&stop));
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        return;
                    }
                    if let Ok(stream) = stream {
                        handle(stream, &routes, &log);
                    }
                }
            })
        };
        Self {
            addr,
            log,
            stop,
            thread: Some(thread),
        }
    }

    pub fn base(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn requests(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn handle(mut stream: TcpStream, routes: &[Route], log: &Mutex<Vec<String>>) {
    let mut request = Vec::new();
    let mut buf = [0u8; 1024];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => request.extend_from_slice(&buf[..n]),
        }
    }
    let text = String::from_utf8_lossy(&request);
    let path = text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_string();
    log.lock().unwrap().push(path.clone());
    let (status, body) = routes
        .iter()
        .find(|r| r.path == path)
        .map_or((404, b"not found".to_vec()), |r| (r.status, r.body.clone()));
    let head = format!(
        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&body);
}
