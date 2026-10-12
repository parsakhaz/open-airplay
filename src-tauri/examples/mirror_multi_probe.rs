//! Drives the app's mirror controller through the multi-device journey against a real sender:
//! `cargo run --example mirror_multi_probe -- <sender exe> <target A>:<port>:<code> <target B>:<port>:<code> [sender args...]`
//! Starts A (a wrong code first, then its code), adds B while A mirrors (its code), removes A after
//! both mirror, keeps B going, then Stop all. Each snapshot is printed with the time.
//! For a local check, run two `doubletake-test-receiver`s, e.g. on 127.0.0.1 (-auth pin) and
//! 127.0.0.2 (-auth password). Set MIRROR_LOG to a file to keep the sender's log.
use std::sync::mpsc;
use std::time::{Duration, Instant};

use open_airplay_lib::mirror::{Mirror, MirrorPhase, MirrorSnapshot, SenderCommand};

struct Tv {
    target: String,
    port: Option<u16>,
    codes: Vec<String>,
}

fn tv(arg: &str, wrong_first: bool) -> Tv {
    let mut parts = arg.splitn(3, ':');
    let target = parts.next().expect("target").to_string();
    let port = parts.next().and_then(|p| p.parse().ok());
    let code = parts.next().unwrap_or_default().to_string();
    let codes = if wrong_first { vec![code, "0000".into()] } else { vec![code] };
    Tv { target, port, codes }
}

fn phase(s: &MirrorSnapshot, target: &str) -> Option<MirrorPhase> {
    s.targets.iter().find(|t| t.target.as_deref() == Some(target)).map(|t| t.phase)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let usage = "usage: mirror_multi_probe <sender> <A ip:port:code> <B ip:port:code> [sender args]";
    let mut sender_args = vec!["-stdio".to_string()];
    sender_args.extend(args.iter().skip(4).cloned());
    let sender = SenderCommand { program: args.get(1).expect(usage).into(), args: sender_args, path_prepend: vec![], log: std::env::var_os("MIRROR_LOG").map(Into::into) };
    let mut a = tv(args.get(2).expect(usage), true);
    let mut b = tv(args.get(3).expect(usage), false);

    let started = Instant::now();
    let (tx, rx) = mpsc::channel::<MirrorSnapshot>();
    let tx = std::sync::Mutex::new(tx);
    let mirror = Mirror::new(move |s| drop(tx.lock().unwrap().send(s.clone())), || 0);
    let log = |what: &str, s: &MirrorSnapshot| {
        let rows: Vec<String> = s.targets.iter().map(|t| format!("{}={:?}{}", t.target.as_deref().unwrap_or("?"), t.phase, t.reason.as_deref().map(|r| format!(" ({r})")).unwrap_or_default())).collect();
        println!("{:6.2}s {what:<10} [{}]", started.elapsed().as_secs_f32(), rows.join(", "));
    };

    mirror.start(&sender, &a.target, a.port, "TV A", Some(0)).expect("start A");
    let mut b_started = false;
    let mut both_since = None;
    let deadline = Instant::now() + Duration::from_secs(90);
    while Instant::now() < deadline {
        let Ok(s) = rx.recv_timeout(Duration::from_millis(200)) else {
            if let Some(since) = both_since {
                if Instant::now().duration_since(since) > Duration::from_secs(4) {
                    break;
                }
            }
            continue;
        };
        log("snapshot", &s);
        for tv in [&mut a, &mut b] {
            if phase(&s, &tv.target) == Some(MirrorPhase::Code) {
                let code = tv.codes.pop().expect("asked for more codes than given");
                println!("         code for {} -> {code}", tv.target);
                mirror.submit_code(Some(&tv.target), &code).expect("code");
            }
        }
        if phase(&s, &a.target) == Some(MirrorPhase::Mirroring) && !b_started {
            b_started = true;
            println!("         adding {} while {} mirrors", b.target, a.target);
            mirror.start(&sender, &b.target, b.port, "TV B", Some(0)).expect("start B");
        }
        if phase(&s, &a.target) == Some(MirrorPhase::Mirroring) && phase(&s, &b.target) == Some(MirrorPhase::Mirroring) {
            both_since.get_or_insert_with(Instant::now);
        }
        if s.targets.iter().any(|t| t.phase == MirrorPhase::Error) {
            println!("         a TV failed; stopping");
            break;
        }
    }
    let removed = mirror.stop(Some(&a.target)).expect("remove A");
    log("remove A", &removed);
    std::thread::sleep(Duration::from_secs(4));
    log("B alone", &mirror.snapshot());
    let all = mirror.stop(None).expect("stop all");
    log("stop all", &all);
    mirror.shutdown();
}
