//! The small amount of URL handling rnm needs: origins, redirect targets, and
//! safe display.

use std::fmt;

use ureq::http::Uri;

const HTTP: &str = "http";
const HTTPS: &str = "https";
const HTTP_DEFAULT_PORT: u16 = 80;
const HTTPS_DEFAULT_PORT: u16 = 443;

/// An absolute `http` or `https` URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Url {
  uri: Uri,
}

/// Why a string is not a usable URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidUrl(pub String);

impl fmt::Display for InvalidUrl {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(
      f,
      "'{}' is not an http or https URL",
      redact_userinfo(&self.0)
    )
  }
}

impl std::error::Error for InvalidUrl {}

/// The part of a URL that decides where a credential may be sent: scheme,
/// lowercase host, and port (with the scheme's default filled in).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Origin {
  pub scheme: String,
  pub host: String,
  pub port: u16,
}

impl Url {
  /// Parses an absolute `http` or `https` URL.
  ///
  /// # Errors
  ///
  /// Returns [`InvalidUrl`] for anything else, including relative URLs.
  pub fn parse(text: &str) -> Result<Self, InvalidUrl> {
    let invalid = || InvalidUrl(text.to_owned());
    let uri: Uri = text.parse().map_err(|_| invalid())?;
    let has_supported_scheme = matches!(uri.scheme_str(), Some(HTTP | HTTPS));
    let has_host = uri.host().is_some_and(|host| !host.is_empty());
    if has_supported_scheme && has_host {
      Ok(Self { uri })
    } else {
      Err(invalid())
    }
  }

  /// The parsed URI, for the HTTP client.
  #[must_use]
  pub fn uri(&self) -> &Uri {
    &self.uri
  }

  #[must_use]
  pub fn origin(&self) -> Origin {
    let scheme = self.uri.scheme_str().unwrap_or_default().to_owned();
    let default_port = if scheme == HTTPS {
      HTTPS_DEFAULT_PORT
    } else {
      HTTP_DEFAULT_PORT
    };
    Origin {
      host: self.uri.host().unwrap_or_default().to_ascii_lowercase(),
      port: self.uri.port_u16().unwrap_or(default_port),
      scheme,
    }
  }

  #[must_use]
  pub fn path(&self) -> &str {
    self.uri.path()
  }

  #[must_use]
  pub fn is_https(&self) -> bool {
    self.uri.scheme_str() == Some(HTTPS)
  }

  /// `localhost` or a loopback IP. Traffic to it never leaves the machine.
  #[must_use]
  pub fn is_loopback(&self) -> bool {
    let host = self.origin().host;
    let unbracketed = host.trim_start_matches('[').trim_end_matches(']');
    host == "localhost"
      || unbracketed
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
  }

  /// Resolves a `Location` header against this URL (RFC 3986 §5.2, for the
  /// forms servers send: absolute, scheme-relative, absolute-path, and
  /// relative-path).
  ///
  /// # Errors
  ///
  /// Returns [`InvalidUrl`] if the result is not an `http` or `https` URL.
  pub fn join(&self, location: &str) -> Result<Self, InvalidUrl> {
    if location.contains("://") {
      return Self::parse(location);
    }
    let scheme = self.uri.scheme_str().unwrap_or(HTTPS);
    if location.starts_with("//") {
      return Self::parse(&format!("{scheme}:{location}"));
    }
    let authority = self
      .uri
      .authority()
      .map(ToString::to_string)
      .unwrap_or_default();
    if location.starts_with('/') {
      return Self::parse(&format!("{scheme}://{authority}{location}"));
    }
    let directory = self
      .path()
      .rsplit_once('/')
      .map_or("", |(directory, _)| directory);
    Self::parse(&format!("{scheme}://{authority}{directory}/{location}"))
  }

  /// Appends `relative` to this URL's path, adding a `/` between them when
  /// needed: `https://h/dist` + `v20.0.0/SHASUMS256.txt`.
  ///
  /// # Errors
  ///
  /// Returns [`InvalidUrl`] if the result does not parse.
  pub fn child(&self, relative: &str) -> Result<Self, InvalidUrl> {
    let base = self.to_string();
    let base = base.trim_end_matches('/');
    Self::parse(&format!("{base}/{}", relative.trim_start_matches('/')))
  }
}

/// A user name and password taken from a URL (`https://user:pass@host/`),
/// percent-decoded.
#[derive(Clone, PartialEq, Eq)]
pub struct UserInfo {
  pub user: String,
  pub password: String,
}

impl fmt::Debug for UserInfo {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("UserInfo")
      .field("user", &self.user)
      .finish_non_exhaustive()
  }
}

impl Url {
  /// This URL without its `user:password@` part, and that part decoded.
  /// rnm sends it as an HTTP Basic credential scoped to the URL, never in
  /// the URL itself, so it cannot follow a redirect or show up in a log.
  #[must_use]
  pub fn split_userinfo(&self) -> (Self, Option<UserInfo>) {
    let text = self.uri.to_string();
    let stripped = redact_userinfo(&text);
    let userinfo = self.uri.authority().and_then(|authority| {
      let (userinfo, _) = authority.as_str().rsplit_once('@')?;
      let (user, password) = userinfo.split_once(':').unwrap_or((userinfo, ""));
      let decode = |part: &str| {
        percent_encoding::percent_decode_str(part)
          .decode_utf8_lossy()
          .into_owned()
      };
      Some(UserInfo {
        user: decode(user),
        password: decode(password),
      })
    });
    let url = Self::parse(&stripped).unwrap_or_else(|_| self.clone());
    (url, userinfo)
  }
}

/// Displays the URL without any `user:password@` part.
impl fmt::Display for Url {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(&redact_userinfo(&self.uri.to_string()))
  }
}

fn redact_userinfo(text: &str) -> String {
  let Some((scheme, rest)) = text.split_once("://") else {
    return text.to_owned();
  };
  let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
  match rest[..authority_end].rsplit_once('@') {
    | Some((_, host)) => format!("{scheme}://{host}{}", &rest[authority_end..]),
    | None => text.to_owned(),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn url(text: &str) -> Url {
    Url::parse(text).unwrap()
  }

  #[test]
  fn parses_only_absolute_http_urls() {
    assert!(Url::parse("https://nodejs.org/dist").is_ok());
    assert!(Url::parse("http://127.0.0.1:8080/").is_ok());
    for bad in [
      "",
      "nodejs.org/dist",
      "/dist",
      "ftp://x/",
      "file:///tmp",
      "https://",
    ] {
      assert!(Url::parse(bad).is_err(), "{bad:?}");
    }
  }

  #[test]
  fn origins_fill_default_ports() {
    assert_eq!(
      url("https://Mirror.Corp/x").origin(),
      Origin {
        scheme: "https".into(),
        host: "mirror.corp".into(),
        port: 443
      }
    );
    assert_eq!(url("http://mirror.corp/x").origin().port, 80);
    assert_eq!(url("https://mirror.corp:8443/").origin().port, 8443);
    assert_eq!(
      url("https://mirror.corp/").origin(),
      url("https://mirror.corp:443/a").origin()
    );
  }

  #[test]
  fn joins_redirect_locations() {
    let base = url("https://a.example/dist/v20/index.json?x=1");
    assert_eq!(
      base.join("https://b.example/y").unwrap().to_string(),
      "https://b.example/y"
    );
    assert_eq!(
      base.join("//c.example/z").unwrap().to_string(),
      "https://c.example/z"
    );
    assert_eq!(
      base.join("/root").unwrap().to_string(),
      "https://a.example/root"
    );
    assert_eq!(
      base.join("other.json").unwrap().to_string(),
      "https://a.example/dist/v20/other.json"
    );
    assert!(base.join("ftp://x/").is_err());
  }

  #[test]
  fn appends_children() {
    assert_eq!(
      url("https://h/dist")
        .child("index.json")
        .unwrap()
        .to_string(),
      "https://h/dist/index.json"
    );
    assert_eq!(
      url("https://h/dist/").child("/v1/x").unwrap().to_string(),
      "https://h/dist/v1/x"
    );
  }

  #[test]
  fn detects_loopback() {
    for host in [
      "http://localhost:1/",
      "http://127.0.0.1/",
      "http://[::1]:8/",
    ] {
      assert!(url(host).is_loopback(), "{host}");
    }
    assert!(!url("http://mirror.corp/").is_loopback());
  }

  #[test]
  fn splits_userinfo() {
    let (bare, info) = url("https://me%40corp:p%3Ass@h.example/dist").split_userinfo();
    assert_eq!(bare.uri().to_string(), "https://h.example/dist");
    let info = info.unwrap();
    assert_eq!(
      (info.user.as_str(), info.password.as_str()),
      ("me@corp", "p:ss")
    );
    assert!(!format!("{info:?}").contains("p:ss"));
    let (same, none) = url("https://h.example/dist").split_userinfo();
    assert_eq!(same, url("https://h.example/dist"));
    assert!(none.is_none());
  }

  #[test]
  fn hides_userinfo() {
    assert_eq!(
      url("https://user:secret@h.example/a?b").to_string(),
      "https://h.example/a?b"
    );
    assert_eq!(
      InvalidUrl("ftp://u:p@h/x".into()).to_string(),
      "'ftp://h/x' is not an http or https URL"
    );
  }
}
