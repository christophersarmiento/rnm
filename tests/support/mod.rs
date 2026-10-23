//! A tiny HTTP/1.1 server for tests. Each server runs on its own port, so two
//! servers are two different origins.

#![allow(dead_code, reason = "each test binary uses a different subset")]

use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// One request as the server saw it.
#[derive(Clone, Debug)]
pub struct Request {
  pub path: String,
  headers: Vec<(String, String)>,
}

impl Request {
  /// The value of header `name`, compared case-insensitively.
  #[must_use]
  pub fn header(&self, name: &str) -> Option<&str> {
    self
      .headers
      .iter()
      .find(|(key, _)| key.eq_ignore_ascii_case(name))
      .map(|(_, value)| value.as_str())
  }
}

/// Every request a server received, in order.
pub type Seen = Arc<Mutex<Vec<Request>>>;

/// A canned response.
pub struct Reply {
  pub status: u16,
  pub headers: Vec<(&'static str, String)>,
  pub body: Vec<u8>,
}

impl Reply {
  /// 200 with `body` and the `ETag` `"v1"`.
  #[must_use]
  pub fn ok(body: &str) -> Self {
    Self {
      status: 200,
      headers: vec![("ETag", "\"v1\"".to_owned())],
      body: body.as_bytes().to_vec(),
    }
  }

  /// 200 with a binary `body` and its length, like a static file server.
  #[must_use]
  pub fn file(body: &[u8]) -> Self {
    Self {
      status: 200,
      headers: Vec::new(),
      body: body.to_vec(),
    }
  }

  /// 302 to `location`.
  #[must_use]
  pub fn redirect(location: String) -> Self {
    Self {
      status: 302,
      headers: vec![("Location", location)],
      body: Vec::new(),
    }
  }

  /// `status` with an empty body.
  #[must_use]
  pub fn status(status: u16) -> Self {
    Self {
      status,
      headers: Vec::new(),
      body: Vec::new(),
    }
  }
}

/// Starts a server that answers each request with `route(request)`. Returns
/// its base URL and the log of requests.
pub fn serve(route: impl Fn(&Request) -> Reply + Send + 'static) -> (String, Seen) {
  let (base, seen, _) = spawn(None, route);
  (base, seen)
}

/// Like [`serve`], but stops listening after `count` requests. Join the
/// handle to be sure the port is closed.
pub fn serve_count(
  count: usize,
  route: impl Fn(&Request) -> Reply + Send + 'static,
) -> (String, Seen, JoinHandle<()>) {
  spawn(Some(count), route)
}

fn spawn(
  count: Option<usize>,
  route: impl Fn(&Request) -> Reply + Send + 'static,
) -> (String, Seen, JoinHandle<()>) {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let base = format!("http://{}", listener.local_addr().unwrap());
  let seen: Seen = Arc::default();
  let log = Arc::clone(&seen);
  let handle = std::thread::spawn(move || {
    let streams = listener
      .incoming()
      .flatten()
      .take(count.unwrap_or(usize::MAX));
    for stream in streams {
      let request = read_request(&stream);
      log.lock().unwrap().push(request.clone());
      write_reply(stream, &route(&request));
    }
  });
  (base, seen, handle)
}

/// A port nothing listens on. Port 1 is outside every OS's ephemeral
/// range (and privileged on Unix), so no test server bound to port 0 can
/// take it, unlike a port freed after `bind("127.0.0.1:0")`, which a
/// parallel test may receive next.
#[must_use]
pub fn unused_port() -> u16 {
  1
}

fn read_request(stream: &TcpStream) -> Request {
  let mut reader = BufReader::new(stream);
  let mut request_line = String::new();
  reader.read_line(&mut request_line).unwrap();
  let path = request_line
    .split_whitespace()
    .nth(1)
    .unwrap_or_default()
    .to_owned();
  let mut headers = Vec::new();
  loop {
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
      break;
    }
    if let Some((name, value)) = line.split_once(':') {
      headers.push((name.trim().to_owned(), value.trim().to_owned()));
    }
  }
  Request { path, headers }
}

fn write_reply(mut stream: TcpStream, reply: &Reply) {
  let mut head = format!(
    "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n",
    reply.status,
    reply.body.len()
  );
  for (name, value) in &reply.headers {
    let _ = write!(head, "{name}: {value}\r\n");
  }
  let _ = stream.write_all(format!("{head}\r\n").as_bytes());
  let _ = stream.write_all(&reply.body);
}
