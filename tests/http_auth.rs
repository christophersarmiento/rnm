//! Credential scoping against real local HTTP servers.
//!
//! Two servers on different ports are different origins, so they stand in
//! for "the mirror" and "some other host" (such as a CDN a mirror redirects
//! to). Each server records which requests carried an `Authorization` header.

mod support;

use std::ffi::OsString;
use std::io::Read;

use rnm::http::{Client, Conditional, HttpError, Mirror, Url};
use support::{Reply, Seen, serve, unused_port};

const TOKEN: &str = "Bearer s3cret";

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
    .find(|request| request.path == path)
    .and_then(|request| request.header("authorization").map(str::to_owned))
}

#[test]
fn sends_credential_to_the_mirror_and_reads_etag() {
  let (mirror, seen) = serve(|_| Reply::ok("[]"));
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
  let (mirror, seen) = serve(|request| match request.path.as_str() {
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
  let (cdn, cdn_seen) = serve(|_| Reply::ok("tarball"));
  let cdn_file = format!("{cdn}/node.tar.xz");
  let (mirror, seen) = serve(move |request| match request.path.as_str() {
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
  let (other, seen) = serve(|_| Reply::ok("ok"));
  let (mirror, _) = serve(|_| Reply::ok("ok"));
  let client = mirror_client(&format!("{mirror}/dist"), Some(TOKEN));
  get(&client, &format!("{other}/dist/index.json")).unwrap();
  assert_eq!(authorization_for(&seen, "/dist/index.json"), None);
}

#[test]
fn explains_auth_failures() {
  let (mirror, _) =
    serve(
      |request| match (request.path.as_str(), request.header("authorization")) {
        | ("/dist/forbidden", _) => Reply::status(403),
        | (_, Some(TOKEN)) => Reply::ok("ok"),
        | _ => Reply::status(401),
      },
    );
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
  let (mirror, _) = serve(|request| match request.path.as_str() {
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
  let unused_port = unused_port();
  let client = mirror_client(&format!("http://127.0.0.1:{unused_port}/dist"), None);
  let error = get(
    &client,
    &format!("http://127.0.0.1:{unused_port}/dist/index.json"),
  )
  .unwrap_err();
  assert!(matches!(error, HttpError::Transport { .. }), "{error}");
}

#[test]
fn revalidates_with_etag() {
  let (mirror, seen) = serve(|request| match request.header("if-none-match") {
    | Some("\"v1\"") => Reply::status(304),
    | _ => Reply::ok("fresh"),
  });
  let client = mirror_client(&format!("{mirror}/dist"), Some(TOKEN));
  let url = Url::parse(&format!("{mirror}/dist/index.json")).unwrap();

  let Conditional::Modified(mut response) = client.get_if_none_match(&url, None).unwrap() else {
    panic!("expected a full response without an ETag");
  };
  let mut body = String::new();
  response.body.read_to_string(&mut body).unwrap();
  assert_eq!(body, "fresh");

  let revalidated = client
    .get_if_none_match(&url, response.etag.as_deref())
    .unwrap();
  assert!(matches!(revalidated, Conditional::NotModified));
  let seen = seen.lock().unwrap();
  assert_eq!(seen[0].header("if-none-match"), None);
  assert_eq!(seen[1].header("if-none-match"), Some("\"v1\""));
  assert_eq!(seen[1].header("authorization"), Some(TOKEN));
}

#[test]
fn unexpected_not_modified_is_an_error() {
  let (mirror, _) = serve(|_| Reply::status(304));
  let client = mirror_client(&format!("{mirror}/dist"), None);
  let error = get(&client, &format!("{mirror}/dist/index.json")).unwrap_err();
  assert!(
    matches!(error, HttpError::Status { status: 304, .. }),
    "{error}"
  );
}
