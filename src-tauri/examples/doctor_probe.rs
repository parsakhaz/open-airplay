//! Prints what the network doctor sees on this machine:
//! `cargo run --example doctor_probe -- C:\path\to\uxplay.exe`
fn main() {
    let engine = std::env::args().nth(1).expect("usage: doctor_probe <engine exe>");
    let ip = open_airplay_lib::netwatch::primary_ip().map(|i| i.to_string());
    let snapshot = open_airplay_lib::doctor::probe(&engine, ip.as_deref(), None);
    println!("primary ip: {ip:?}\nsnapshot: {snapshot:#?}");
    if let Some(s) = snapshot {
        println!("findings: {:#?}", open_airplay_lib::doctor::diagnose(&s));
    }
}
