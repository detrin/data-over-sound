use dos::channel::ChannelData;
use dos_wasm::sim::Air;
use dos_wasm::station::{config, wrap_envelope, Role, State, Station, FLAG_MESSAGE};

const FS: usize = 48_000;
const BLOCK: usize = 960;

struct Run {
    delivered: Option<dos_wasm::station::Delivered>,
    seconds: f64,
    leader_state: State,
}

fn exchange(f_lo: f64, f_hi: f64, data: Vec<u8>, scenario: Option<usize>, max_s: f64) -> Run {
    let ab = ChannelData::load("dell-g5_to_e540").unwrap();
    let ba = ChannelData::load("e540_to_dell-g5").unwrap();
    let mut air = Air::new(ab, ba, FS, 3);
    if let Some(i) = scenario {
        air.interferers = dos::scenario::phases()[i].1.clone();
    }
    let stream = wrap_envelope("notes.txt", "text/plain", 0, &data);
    let mut st = [
        Station::new(config(f_lo, f_hi), Role::Leader, Some(stream), 1).unwrap(),
        Station::new(config(f_lo, f_hi), Role::Follower, None, 2).unwrap(),
    ];
    let mut playing = [0usize; 2];
    let mut t = 0usize;
    let mut delivered = None;
    while (t as f64 / FS as f64) < max_s {
        let now = t as f64 * 1000.0 / FS as f64;
        for i in 0..2 {
            let heard = air.hear(i, BLOCK);
            st[i].push_audio(&heard);
            if playing[i] == 0 {
                if let Some(w) = st[i].poll(now) {
                    air.transmit(i, &w);
                    playing[i] = w.len();
                }
            }
        }
        t += BLOCK;
        for i in 0..2 {
            if playing[i] > 0 {
                playing[i] = playing[i].saturating_sub(BLOCK);
                if playing[i] == 0 {
                    st[i].tx_done(t as f64 * 1000.0 / FS as f64);
                }
            }
        }
        if delivered.is_none() {
            delivered = st[1].take_completed();
        }
        if delivered.is_some() && st[0].status().state == State::Done {
            break;
        }
    }
    Run { delivered, seconds: t as f64 / FS as f64, leader_state: st[0].status().state }
}

fn bytes(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i * 131 % 251) as u8).collect()
}

#[test]
fn file_crosses_the_measured_room_on_the_dos_band() {
    let data = bytes(6_000);
    let r = exchange(800.0, 10_000.0, data.clone(), None, 400.0);
    let d = r.delivered.expect("follower should rebuild the file");
    assert_eq!(d.data, data);
    assert_eq!(d.name, "notes.txt");
    assert_eq!(r.leader_state, State::Done, "leader should see everything acknowledged");
    eprintln!("6000 B in {:.0} s of simulated air", r.seconds);
}

#[test]
fn transfer_survives_a_jammer() {
    let data = bytes(2_000);
    let r = exchange(800.0, 10_000.0, data.clone(), Some(1), 400.0);
    assert_eq!(r.delivered.expect("delivered under a 2-4 kHz jammer").data, data);
}

#[test]
fn custom_band_works_with_generated_tables() {
    let data = bytes(500);
    let r = exchange(4_000.0, 9_000.0, data.clone(), None, 300.0);
    assert_eq!(r.delivered.expect("delivered on 4-9 kHz").data, data);
}

#[test]
fn envelope_round_trip_and_raw_fallback() {
    let w = wrap_envelope("ü.txt", "text/plain", FLAG_MESSAGE, b"hi");
    let d = dos_wasm::station::unwrap_envelope(&w);
    assert_eq!((d.name.as_str(), d.mime.as_str(), d.flags, d.data.as_slice()), ("ü.txt", "text/plain", FLAG_MESSAGE, &b"hi"[..]));
    let raw = dos_wasm::station::unwrap_envelope(b"plain bytes from dos duplex");
    assert_eq!(raw.data, b"plain bytes from dos duplex");
}

#[test]
fn band_validation() {
    use dos_wasm::station::validate;
    assert!(validate(&config(800.0, 10_000.0)).is_ok());
    assert!(validate(&config(17_000.0, 17_400.0)).is_err(), "too narrow for the header");
    assert!(validate(&config(800.0, 20_000.0)).is_err(), "too many tiles for the header maps");
}
