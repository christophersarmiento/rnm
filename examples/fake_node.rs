//! A stand-in `node.exe` for the Windows shell tests: prints the name of the
//! version directory it was copied into (`v20.19.0`), seen through any
//! junction.

fn main() {
  let exe = std::env::current_exe().expect("own path");
  let real = std::fs::canonicalize(&exe).unwrap_or(exe);
  let version = real
    .parent()
    .and_then(std::path::Path::file_name)
    .expect("a version directory");
  println!("{}", version.to_string_lossy());
}
