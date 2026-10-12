//! Drives the app's mirror controller (the same code behind Mirror screen) from the command line:
//! `cargo run --example mirror_probe -- <sender exe> <target> [port] [codes,comma,separated]`
//! Each time the TV asks for a code, the next one is sent. After 5 s of mirroring it stops.
//! For a local check, point it at `doubletake-test-receiver -auth pin -code 4321`.
use std::sync::mpsc;
use std::time::{Duration, Instant};

use open_airplay_lib::mirror::{Mirror, MirrorPhase, MirrorSnapshot, SenderCommand};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let sender = SenderCommand { program: args.get(1).expect("usage: mirror_probe <sender> <target> [port] [codes]").into(), args: vec!["-stdio".into()], path_prepend: vec![], log: None };
    let target = args.get(2).expect("target").clone();
    let port = args.get(3).and_then(|p| p.parse().ok());
    let mut codes = args.get(4).map(|c| c.split(',').map(str::to_string).collect::<Vec<_>>()).unwrap_or_default().into_iter();

    let started = Instant::now();
    let (tx, rx) = mpsc::channel::<MirrorSnapshot>();
    let tx = std::sync::Mutex::new(tx);
    let mirror = Mirror::new(move |s| drop(tx.lock().unwrap().send(s.clone())), || 0);
    mirror.start(&sender, &target, port, "Test TV", Some(0)).expect("start");
    let mut mirroring_since = None;
    loop {
        if let Some(since) = mirroring_since {
            if Instant::now().duration_since(since) > Duration::from_secs(5) {
                let t = Instant::now();
                let after = mirror.stop(None).expect("stop");
                println!("{:6.2}s stop took {:.2}s -> {after:?}", started.elapsed().as_secs_f32(), t.elapsed().as_secs_f32());
                break;
            }
        }
        let Ok(snapshot) = rx.recv_timeout(Duration::from_millis(200)) else { continue };
        println!("{:6.2}s {snapshot:?}", started.elapsed().as_secs_f32());
        let Some(status) = snapshot.targets.first() else { continue };
        match status.phase {
            MirrorPhase::Code => {
                let code = codes.next().expect("the TV asked for more codes than given");
                println!("        sending code {code}");
                mirror.submit_code(None, &code).expect("code");
            }
            MirrorPhase::Mirroring => {
                mirroring_since.get_or_insert_with(Instant::now);
            }
            MirrorPhase::Error => break,
            _ => {}
        }
    }
    mirror.shutdown();
}
