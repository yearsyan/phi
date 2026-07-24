use std::{env, fs, path::Path};

/// Placeholder served when the daemon was compiled without a real web build.
const PLACEHOLDER_INDEX: &str = r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>phi</title>
  </head>
  <body>
    <p>The phi web client is not embedded in this daemon binary.</p>
    <p>
      Build it with <code>cd web &amp;&amp; pnpm install &amp;&amp; pnpm build</code>,
      then rebuild <code>phi-daemon</code> to embed <code>web/dist</code>.
    </p>
  </body>
</html>
"#;

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set");
    let dist = Path::new(&manifest_dir).join("../../web/dist");

    // rust-embed requires the folder to exist at compile time, so keep a
    // minimal placeholder for checkouts where `pnpm build` has not run yet.
    if !dist.exists() {
        fs::create_dir_all(&dist).expect("create placeholder web/dist directory");
        fs::write(dist.join("index.html"), PLACEHOLDER_INDEX)
            .expect("write placeholder web/dist/index.html");
    }

    // Rebuild the daemon when the embedded web assets change. Cargo only
    // watches directories shallowly, so list every file explicitly.
    println!("cargo:rerun-if-changed={}", dist.display());
    watch_directory(&dist);
}

fn watch_directory(directory: &Path) {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            println!("cargo:rerun-if-changed={}", path.display());
            watch_directory(&path);
        } else {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}
