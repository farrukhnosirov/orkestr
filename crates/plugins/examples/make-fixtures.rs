//! Writes the plugin fixture packages (`hello-tool.orkp`, `evil-*.orkp`),
//! `dev-publisher.pub` and `fixtures.json` into the given directory.
//!
//! `cargo run -p orkestr-plugins --example make-fixtures -- <outdir>`

fn main() {
    let Some(dir) = std::env::args().nth(1) else {
        eprintln!("usage: make-fixtures <outdir>");
        std::process::exit(2);
    };
    match orkestr_plugins::fixtures::write_all(std::path::Path::new(&dir)) {
        Ok(files) => {
            for f in files {
                println!("{}", f.display());
            }
        }
        Err(e) => {
            eprintln!("make-fixtures: {e}");
            std::process::exit(1);
        }
    }
}
