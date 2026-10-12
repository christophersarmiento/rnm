//! Credential scoping against real local HTTP servers.
//!
//! Two servers on different ports are different origins, so they stand in
//! for "the mirror" and "some other host" (such as a CDN a mirror redirects
//! to). Each server records which requests carried an `Authorization` header.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Read, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use rnm::http::{Client, HttpError, Mirror, Url};

const TOKEN: &str = "Bearer s3cret";

/// What one request looked like: its path and its `Authorization` header.
type Seen = Arc<Mutex<Vec<(String, Option<String>)>>>;

/// A canned response.
struct Reply {
  status: u16,
  headers: Vec<(&'static str, String)>,
  body: String,
}

impl Reply {
  fn ok(body: &str) -> Self {
    Self {
      status: 200,
      headers: vec![("ETag", "\"v1\"".to_owned())],
      body: body.to_owned(),
    }
  }

  fn redirect(location: String) -> Self {
    Self {
      status: 302,
      headers: vec![("Location", location)],
      body: String::new(),
    }
  }

  fn status(status: u16) -> Self {
    Self {
      status,
      headers: Vec::new(),
      body: String::new(),
    }
  }
}

/// Starts a server that answers with `route(path, authorization)`. Returns
/// its base URL and the log of requests.
fn serve(route: impl Fn(&str, Option<&str>) -> Reply + Send + 'static) -> (String, Seen) {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let base = format!("http://{}", listener.local_addr().unwrap());
  let seen: Seen = Arc::default();
  let log = Arc::clone(&seen);
  std::thread::spawn(move || {
    for stream in listener.incoming().flatten() {
      let (path, authorization) = read_request(&stream);
      log
        .lock()
        .unwrap()
        .push((path.clone(), authorization.clone()));
      write_reply(stream, &route(&path, authorization.as_deref()));
    }
  });
  (base, seen)
}

fn read_request(stream: &TcpStream) -> (String, Option<String>) {
  let mut reader = BufReader::new(stream);
  let mut request_line = String::new();
  reader.read_line(&mut request_line).unwrap();
  let path = request_line
    .split_whitespace()
    .nth(1)
    .unwrap_or_default()
    .to_owned();
  let mut authorization = None;
  loop {
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
      break;
    }
    if let Some((name, value)) = line.split_once(':')
      && name.eq_ignore_ascii_case("authorization")
    {
      authorization = Some(value.trim().to_owned());
    }
  }
  (path, authorization)
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
  let _ = stream.write_all(format!("{head}\r\n{}", reply.body).as_bytes());
}

fn mirror_client(mirror_url: &str, token: Option<&str>) -> Client {
  let vars: Vec<(&str, String)> = [
    ("RNM_NODE_MIRROR", Some(mirror_url.to_owned())),
    ("RNM_AUTH_HEADER", token.map(str::to_owned)),
  ]
  .into_iter()
  .filter_map(|(key, value)| Some((key, value?)))
  .collect();
  let mirror = Mirror::from_env(|key| {
    vars
      .iter()
      .find(|(k, _)| *k == key)
      .map(|(_, v)| OsString::from(v))
  })
  .unwrap();
  Client::for_mirror(&mirror)
}

fn get(client: &Client, url: &str) -> Result<(String, Option<String>), HttpError> {
  let mut response = client.get(&Url::parse(url).unwrap())?;
  let mut body = String::new();
  response.body.read_to_string(&mut body).unwrap();
  Ok((body, response.etag))
}

fn authorization_for(seen: &Seen, path: &str) -> Option<String> {
  seen
    .lock()
    .unwrap()
    .iter()
    .find(|(p, _)| p == path)
    .and_then(|(_, auth)| auth.clone())
}

#[test]
fn sends_credential_to_the_mirror_and_reads_etag() {
  let (mirror, seen) = serve(|_, _| Reply::ok("[]"));
  let client = mirror_client(&format!("{mirror}/dist"), Some(TOKEN));
  let (body, etag) = get(&client, &format!("{mirror}/dist/index.json")).unwrap();
  assert_eq!(body, "[]");
  assert_eq!(etag.as_deref(), Some("\"v1\""));
  assert_eq!(
    authorization_for(&seen, "/dist/index.json").as_deref(),
    Some(TOKEN)
  );
}

#[test]
fn keeps_credential_on_same_scope_redirects() {
  let (mirror, seen) = serve(|path, _| match path {
    | "/dist/latest" => Reply::redirect("/dist/v20/index.json".to_owned()),
    | _ => Reply::ok("ok"),
  });
  let client = mirror_client(&format!("{mirror}/dist"), Some(TOKEN));
  get(&client, &format!("{mirror}/dist/latest")).unwrap();
  assert_eq!(
    authorization_for(&seen, "/dist/latest").as_deref(),
    Some(TOKEN)
  );
  assert_eq!(
    authorization_for(&seen, "/dist/v20/index.json").as_deref(),
    Some(TOKEN)
  );
}

#[test]
fn drops_credential_when_a_redirect_leaves_the_scope() {
  let (cdn, cdn_seen) = serve(|_, _| Reply::ok("tarball"));
  let cdn_file = format!("{cdn}/node.tar.xz");
  let (mirror, seen) = serve(move |path, _| match path {
    | "/dist/to-cdn" => Reply::redirect(cdn_file.clone()),
    | "/dist/to-outside-path" => Reply::redirect("/private/x".to_owned()),
    | _ => Reply::ok("ok"),
  });
  let client = mirror_client(&format!("{mirror}/dist"), Some(TOKEN));

  assert_eq!(
    get(&client, &format!("{mirror}/dist/to-cdn")).unwrap().0,
    "tarball"
  );
  assert_eq!(
    authorization_for(&seen, "/dist/to-cdn").as_deref(),
    Some(TOKEN)
  );
  assert_eq!(
    authorization_for(&cdn_seen, "/node.tar.xz"),
    None,
    "credential leaked to another origin"
  );

  get(&client, &format!("{mirror}/dist/to-outside-path")).unwrap();
  assert_eq!(
    authorization_for(&seen, "/private/x"),
    None,
    "credential leaked outside the mirror path"
  );
}

#[test]
fn never_sends_credential_outside_the_mirror() {
  let (other, seen) = serve(|_, _| Reply::ok("ok"));
  let (mirror, _) = serve(|_, _| Reply::ok("ok"));
  let client = mirror_client(&format!("{mirror}/dist"), Some(TOKEN));
  get(&client, &format!("{other}/dist/index.json")).unwrap();
  assert_eq!(authorization_for(&seen, "/dist/index.json"), None);
}

#[test]
fn explains_auth_failures() {
  let (mirror, _) = serve(|path, authorization| match (path, authorization) {
    | ("/dist/forbidden", _) => Reply::status(403),
    | (_, Some(TOKEN)) => Reply::ok("ok"),
    | _ => Reply::status(401),
  });
  let url = format!("{mirror}/dist/index.json");

  let error = get(&mirror_client(&format!("{mirror}/dist"), None), &url).unwrap_err();
  assert!(matches!(
    error,
    HttpError::Unauthorized {
      sent_credential_from: None,
      ..
    }
  ));
  assert!(error.to_string().contains("set RNM_AUTH_HEADER"), "{error}");

  let error = get(
    &mirror_client(&format!("{mirror}/dist"), Some("Bearer wrong")),
    &url,
  )
  .unwrap_err();
  assert!(
    error
      .to_string()
      .contains("401 Unauthorized (sent the credential from RNM_AUTH_HEADER)"),
    "{error}"
  );
  assert!(!error.to_string().contains("wrong"));

  let client = mirror_client(&format!("{mirror}/dist"), Some(TOKEN));
  let error = get(&client, &format!("{mirror}/dist/forbidden")).unwrap_err();
  assert!(matches!(error, HttpError::Forbidden { .. }), "{error}");
}

#[test]
fn reports_missing_files_and_redirect_loops() {
  let (mirror, _) = serve(|path, _| match path {
    | "/dist/loop" => Reply::redirect("/dist/loop".to_owned()),
    | "/dist/nowhere" => Reply::redirect("ftp://elsewhere/".to_owned()),
    | "/dist/broken" => Reply::status(502),
    | _ => Reply::status(404),
  });
  let client = mirror_client(&format!("{mirror}/dist"), None);
  let fetch = |path: &str| get(&client, &format!("{mirror}{path}")).unwrap_err();
  assert!(matches!(fetch("/dist/missing"), HttpError::NotFound { .. }));
  assert!(matches!(
    fetch("/dist/loop"),
    HttpError::TooManyRedirects { .. }
  ));
  assert!(matches!(
    fetch("/dist/nowhere"),
    HttpError::BadRedirect { .. }
  ));
  let error = fetch("/dist/broken");
  assert!(
    error.to_string().ends_with("HTTP 502 Bad Gateway"),
    "{error}"
  );
}

#[test]
fn reports_connection_failures() {
  let unused_port = TcpListener::bind("127.0.0.1:0")
    .unwrap()
    .local_addr()
    .unwrap()
    .port();
  let client = mirror_client(&format!("http://127.0.0.1:{unused_port}/dist"), None);
  let error = get(
    &client,
    &format!("http://127.0.0.1:{unused_port}/dist/index.json"),
  )
  .unwrap_err();
  assert!(matches!(error, HttpError::Transport { .. }), "{error}");
}
