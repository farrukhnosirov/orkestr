//! Installs voice packages with the real model manager (checksum-verified):
//!
//! `cargo run -p orkestr-voice --example voice_install -- <ORKESTR_HOME>/voice whisper-base vosk-small-ru …`
#![allow(clippy::print_stdout)]

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("usage: voice_install <voice dir> <package id>…");
    let service = orkestr_voice::VoiceService::new(dir.into());
    for id in args {
        let t = std::time::Instant::now();
        match service.models().install_blocking(&id).await {
            Ok(()) => println!("installed {id} in {:.1} s", t.elapsed().as_secs_f32()),
            Err(e) => println!("FAILED {id}: {e}"),
        }
    }
    let list = service.models().list();
    println!("disk usage: {} MB under {}", list.disk_usage_bytes / 1_000_000, list.root);
}
