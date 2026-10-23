//! Colors, glyphs, and spinners for terminal output.
//!
//! Everything stays plain text until [`init`] finds a terminal, so piped
//! output, scripts, and tests keep nvm's exact wording.

use std::fmt::Display;
use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

pub const BOLD: &str = "1";
pub const DIM: &str = "2";
pub const RED: &str = "1;31";
pub const GREEN: &str = "1;32";
pub const YELLOW: &str = "1;33";
pub const CYAN: &str = "36";
pub const BOLD_CYAN: &str = "1;36";

static COLOR_OUT: AtomicBool = AtomicBool::new(false);
static COLOR_ERR: AtomicBool = AtomicBool::new(false);
static ANIMATE_ERR: AtomicBool = AtomicBool::new(false);
static TRUECOLOR: AtomicBool = AtomicBool::new(false);

/// Where a line goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
  Out,
  Err,
}

/// Turns on colors for each standard stream that is a terminal (unless
/// `NO_COLOR` is set or `TERM=dumb`), spinners when stderr is one, and
/// 24-bit colors when `COLORTERM` says the terminal has them.
pub fn init() {
  let var = |key| std::env::var_os(key).filter(|v| !v.is_empty());
  let dumb = var("TERM").is_some_and(|t| t == "dumb");
  let colors = var("NO_COLOR").is_none() && !dumb;
  let err_tty = std::io::stderr().is_terminal();
  COLOR_OUT.store(colors && std::io::stdout().is_terminal(), Ordering::Relaxed);
  COLOR_ERR.store(colors && err_tty, Ordering::Relaxed);
  ANIMATE_ERR.store(err_tty && !dumb, Ordering::Relaxed);
  let truecolor = var("COLORTERM").is_some_and(|v| v == "truecolor" || v == "24bit");
  TRUECOLOR.store(truecolor, Ordering::Relaxed);
}

/// Whether `stream` gets colors.
#[must_use]
pub fn enabled(stream: Stream) -> bool {
  match stream {
    | Stream::Out => COLOR_OUT.load(Ordering::Relaxed),
    | Stream::Err => COLOR_ERR.load(Ordering::Relaxed),
  }
}

/// Whether `stream` gets 24-bit colors.
#[must_use]
pub fn truecolor(stream: Stream) -> bool {
  enabled(stream) && TRUECOLOR.load(Ordering::Relaxed)
}

/// `text` in the SGR `code` when `stream` has colors.
#[must_use]
pub fn paint(stream: Stream, code: &str, text: impl Display) -> String {
  if enabled(stream) {
    format!("\x1b[{code}m{text}\x1b[0m")
  } else {
    text.to_string()
  }
}

/// A finished step: `✓ text`.
#[must_use]
pub fn success(stream: Stream, text: impl Display) -> String {
  if enabled(stream) {
    format!("{} {text}", paint(stream, GREEN, "✓"))
  } else {
    text.to_string()
  }
}

/// A `warning: …` line for stderr, with the label highlighted.
#[must_use]
pub fn warning(text: impl Display) -> String {
  let text = text.to_string();
  if !enabled(Stream::Err) {
    return text;
  }
  let rest = text.strip_prefix("warning:").unwrap_or(&text);
  format!(
    "{} {}{rest}",
    paint(Stream::Err, YELLOW, "⚠"),
    paint(Stream::Err, YELLOW, "warning:")
  )
}

/// A failure message for stderr: `✗ text`.
#[must_use]
pub fn error(text: impl Display) -> String {
  if enabled(Stream::Err) {
    format!("{} {text}", paint(Stream::Err, RED, "✗"))
  } else {
    text.to_string()
  }
}

/// Spinner frames, one every [`FRAME`].
const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const FRAME: Duration = Duration::from_millis(80);
/// Steps faster than this never show a spinner, so nothing flickers.
const DELAY: Duration = Duration::from_millis(150);

/// A spinner on stderr that runs until dropped, then erases itself.
pub struct Spinner {
  running: Option<(mpsc::Sender<()>, JoinHandle<()>)>,
}

impl Spinner {
  /// Starts a spinner with `message`; does nothing when stderr is not a
  /// terminal.
  #[must_use]
  pub fn start(message: impl Into<String>) -> Self {
    if !ANIMATE_ERR.load(Ordering::Relaxed) {
      return Self { running: None };
    }
    let message = message.into();
    let (stop, stopped) = mpsc::channel::<()>();
    let handle = std::thread::spawn(move || {
      if stopped.recv_timeout(DELAY) != Err(mpsc::RecvTimeoutError::Timeout) {
        return;
      }
      let mut err = std::io::stderr();
      for frame in FRAMES.iter().cycle() {
        let _ = write!(
          err,
          "\r{} {message}\x1b[K",
          paint(Stream::Err, BOLD_CYAN, frame)
        );
        let _ = err.flush();
        if stopped.recv_timeout(FRAME) != Err(mpsc::RecvTimeoutError::Timeout) {
          break;
        }
      }
      let _ = write!(err, "\r\x1b[K");
    });
    Self {
      running: Some((stop, handle)),
    }
  }
}

impl Drop for Spinner {
  fn drop(&mut self) {
    if let Some((stop, handle)) = self.running.take() {
      drop(stop);
      let _ = handle.join();
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn plain_until_init() {
    assert_eq!(paint(Stream::Err, RED, "x"), "x");
    assert_eq!(success(Stream::Out, "done"), "done");
    assert_eq!(warning("warning: careful"), "warning: careful");
    assert_eq!(error("bad"), "bad");
    drop(Spinner::start("nothing to see"));
  }
}
