//! netrc files (`~/.netrc`, `%USERPROFILE%\_netrc`): login and password per
//! host, the format curl, git, and Artifactory users already have.
//!
//! The `default` entry is ignored on purpose: it would send a password to
//! every host. `macdef` macros are skipped.

/// A `machine` entry with both a login and a password.
#[derive(Clone, PartialEq, Eq)]
pub struct NetrcEntry {
  pub machine: String,
  pub login: String,
  pub password: String,
}

impl std::fmt::Debug for NetrcEntry {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("NetrcEntry")
      .field("machine", &self.machine)
      .field("login", &self.login)
      .finish_non_exhaustive()
  }
}

/// Parses netrc text. Entries without a login or password are dropped; when
/// a machine appears twice, the first entry wins, as in curl.
#[must_use]
pub fn parse_netrc(text: &str) -> Vec<NetrcEntry> {
  let mut entries: Vec<NetrcEntry> = Vec::new();
  let mut current: Option<Partial> = None;
  let mut tokens = Tokens::new(text);
  while let Some(token) = tokens.next() {
    match token.as_str() {
      | "machine" | "default" => {
        if let Some(entry) = current.take().and_then(Partial::complete) {
          push_unique(&mut entries, entry);
        }
        if token == "machine" {
          current = Some(Partial {
            machine: tokens.next().unwrap_or_default(),
            ..Partial::default()
          });
        }
      },
      | "login" => set(&mut current, &mut tokens, |p, v| p.login = Some(v)),
      | "password" => set(&mut current, &mut tokens, |p, v| p.password = Some(v)),
      | "account" => {
        tokens.next();
      },
      | "macdef" => {
        tokens.next();
        tokens.skip_macro_body();
      },
      | _ => {},
    }
  }
  if let Some(entry) = current.and_then(Partial::complete) {
    push_unique(&mut entries, entry);
  }
  entries
}

fn push_unique(entries: &mut Vec<NetrcEntry>, entry: NetrcEntry) {
  let seen = entries
    .iter()
    .any(|e| e.machine.eq_ignore_ascii_case(&entry.machine));
  if !seen {
    entries.push(entry);
  }
}

fn set(current: &mut Option<Partial>, tokens: &mut Tokens<'_>, apply: fn(&mut Partial, String)) {
  let value = tokens.next().unwrap_or_default();
  if let Some(partial) = current {
    apply(partial, value);
  }
}

#[derive(Default)]
struct Partial {
  machine: String,
  login: Option<String>,
  password: Option<String>,
}

impl Partial {
  fn complete(self) -> Option<NetrcEntry> {
    Some(NetrcEntry {
      login: self.login?,
      password: self.password?,
      machine: self.machine,
    })
    .filter(|entry| !entry.machine.is_empty())
  }
}

/// Whitespace-separated tokens; a token may be `"quoted"` with `\` escapes,
/// as curl allows.
struct Tokens<'a> {
  rest: &'a str,
}

impl<'a> Tokens<'a> {
  fn new(text: &'a str) -> Self {
    Self { rest: text }
  }

  /// A macro body runs until the next empty line.
  fn skip_macro_body(&mut self) {
    match self.rest.find("\n\n") {
      | Some(end) => self.rest = &self.rest[end..],
      | None => self.rest = "",
    }
  }
}

impl Iterator for Tokens<'_> {
  type Item = String;

  fn next(&mut self) -> Option<String> {
    self.rest = self.rest.trim_start();
    // A `#` at the start of a token begins a comment, as in curl.
    while self.rest.starts_with('#') {
      self.rest = self.rest.find('\n').map_or("", |end| &self.rest[end..]);
      self.rest = self.rest.trim_start();
    }
    if self.rest.is_empty() {
      return None;
    }
    if let Some(quoted) = self.rest.strip_prefix('"') {
      let mut token = String::new();
      let mut chars = quoted.char_indices();
      while let Some((i, c)) = chars.next() {
        match c {
          | '"' => {
            self.rest = &quoted[i + 1..];
            return Some(token);
          },
          | '\\' => token.extend(chars.next().map(|(_, c)| c)),
          | c => token.push(c),
        }
      }
      self.rest = "";
      return Some(token);
    }
    let end = self
      .rest
      .find(char::is_whitespace)
      .unwrap_or(self.rest.len());
    let token = self.rest[..end].to_owned();
    self.rest = &self.rest[end..];
    Some(token)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn entry(machine: &str, login: &str, password: &str) -> NetrcEntry {
    NetrcEntry {
      machine: machine.into(),
      login: login.into(),
      password: password.into(),
    }
  }

  #[test]
  fn parses_entries_on_one_or_many_lines() {
    let text = "machine a.corp login u1 password p1\n\
                # a comment\n\
                machine b.corp\n  login u2\n  account x\n  password \"p 2\\\"\"\n";
    assert_eq!(
      parse_netrc(text),
      vec![entry("a.corp", "u1", "p1"), entry("b.corp", "u2", "p 2\"")]
    );
  }

  #[test]
  fn ignores_default_incomplete_and_duplicate_entries() {
    let text = "machine a login u password p\n\
                machine A login v password q\n\
                machine nopass login u\n\
                default login anon password guest\n";
    assert_eq!(parse_netrc(text), vec![entry("a", "u", "p")]);
  }

  #[test]
  fn skips_macros() {
    let text = "macdef init\ncd /pub\nmachine fake login x password y\n\n\
                machine real login u password p\n";
    assert_eq!(parse_netrc(text), vec![entry("real", "u", "p")]);
  }

  #[test]
  fn debug_hides_the_password() {
    assert!(!format!("{:?}", entry("h", "u", "hunter2")).contains("hunter2"));
  }
}
