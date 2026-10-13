//! Measure how long MIDI port enumeration and connection take on this machine.
//! `cargo run --release -p midi-io --example port_timing`
use std::time::Instant;

fn main() {
    let t = Instant::now();
    let input = midi_io::MidiInputManager::new().unwrap();
    let output = midi_io::MidiOutputManager::new().unwrap();
    println!("managers created: {:?}", t.elapsed());

    for round in 0..3 {
        let t = Instant::now();
        let ins = input.inputs();
        let dt_in = t.elapsed();
        let t = Instant::now();
        let outs = output.outputs();
        let dt_out = t.elapsed();
        println!("round {round}: inputs() {dt_in:?} -> {ins:?}");
        println!("round {round}: outputs() {dt_out:?} -> {outs:?}");
    }

    let ins = input.inputs();
    if let Some(port) = ins.iter().find(|p| p.to_string().contains("CLP")).cloned() {
        let t = Instant::now();
        let conn =
            midi_io::MidiInputManager::connect_input(port, |msg| println!("  in: {msg:02x?}"));
        println!(
            "connect_input(CLP): {:?} ok={}",
            t.elapsed(),
            conn.is_some()
        );
        if conn.is_some() {
            println!("play a few keys within 5 s ...");
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
    } else {
        println!("no CLP input port found");
    }

    let outs = output.outputs();
    if let Some(port) = outs.iter().find(|p| p.to_string().contains("CLP")).cloned() {
        let t = Instant::now();
        let conn = midi_io::MidiOutputManager::connect_output(port);
        println!(
            "connect_output(CLP): {:?} ok={}",
            t.elapsed(),
            conn.is_some()
        );
        if let Some(mut conn) = conn {
            conn.send(&[0x90, 60, 80]).ok();
            std::thread::sleep(std::time::Duration::from_millis(500));
            conn.send(&[0x80, 60, 0]).ok();
            println!("sent middle C to the piano");
        }
    }
}
