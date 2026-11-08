//! Interop with the Python reference implementation: golden vectors from interop/golden.py.

use dos::frame::{self, Header};
use dos::link::receive_frame;
use dos::phy::{Phy, PhyConfig};
use dos::rs;
use serde_json::Value;

fn golden(name: &str) -> Value {
    let p = format!("{}/interop/golden/{}", env!("CARGO_MANIFEST_DIR"), name);
    serde_json::from_str(&std::fs::read_to_string(p).expect("run interop/golden.py first")).unwrap()
}

fn read_wav(name: &str) -> Vec<f64> {
    let p = format!("{}/interop/golden/{}", env!("CARGO_MANIFEST_DIR"), name);
    hound::WavReader::open(p).unwrap().samples::<i16>().map(|s| s.unwrap() as f64 / 32768.0).collect()
}

#[test]
fn reed_solomon_matches_reedsolo() {
    for case in golden("rs.json").as_array().unwrap() {
        let nsym = case["nsym"].as_u64().unwrap() as usize;
        let msg = hex::decode(case["msg"].as_str().unwrap()).unwrap();
        let cw = hex::decode(case["cw"].as_str().unwrap()).unwrap();
        let bad = hex::decode(case["corrupted"].as_str().unwrap()).unwrap();
        assert_eq!(rs::encode(&msg, nsym), cw, "encode nsym={nsym}");
        assert_eq!(rs::decode(&bad, nsym).as_deref(), Some(&msg[..]), "decode nsym={nsym}");
    }
}

#[test]
fn header_pack_matches_python() {
    let n = PhyConfig::default().n_tiles();
    for h in golden("header.json").as_array().unwrap() {
        let v = |k: &str| h[k].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u8).collect::<Vec<u8>>();
        let hdr = Header {
            seq: h["seq"].as_u64().unwrap() as u16,
            fb_seq: h["fb_seq"].as_u64().unwrap() as u16,
            levels: v("levels"),
            fb_levels: v("fb_levels"),
            fb_ok: v("fb_ok").iter().map(|&x| x == 1).collect(),
        };
        let packed = hex::decode(h["packed"].as_str().unwrap()).unwrap();
        assert_eq!(hdr.pack(n), packed);
        assert_eq!(Header::unpack(&packed, n), hdr);
    }
}

#[test]
fn sync_and_bins_match_python() {
    let g = golden("sync.json");
    let phy = Phy::new(PhyConfig::default());
    assert_eq!(phy.sync.len() as u64, g["sync_len"].as_u64().unwrap());
    assert_eq!(phy.cfg.n_bins() as u64, g["n_bins"].as_u64().unwrap());
    assert_eq!(phy.cfg.first_bin() as u64, g["bin0"].as_u64().unwrap());
    assert_eq!(phy.cfg.frame_len() as u64, g["frame_len"].as_u64().unwrap());
    for (a, b) in phy.sync.iter().take(8).zip(g["sync_head"].as_array().unwrap()) {
        assert!((a - b.as_f64().unwrap()).abs() < 1e-12);
    }
    for (a, b) in phy.sync.iter().rev().take(8).zip(g["sync_tail"].as_array().unwrap().iter().rev()) {
        assert!((a - b.as_f64().unwrap()).abs() < 1e-12);
    }
}

#[test]
fn rust_decodes_python_frames() {
    let phy = Phy::new(PhyConfig::default());
    for name in ["mixed", "l3", "l1"] {
        let meta = golden(&format!("frame_{name}.json"));
        let rx = read_wav(&format!("frame_{name}.wav"));
        let r = receive_frame(&phy, &rx, None, None);
        let h = r.header.expect("header");
        assert_eq!(h.seq, 77);
        assert_eq!(h.fb_seq, 76);
        let levels: Vec<u8> = meta["levels"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u8).collect();
        assert_eq!(h.levels, levels);
        for (k, p) in meta["payloads"].as_array().unwrap().iter().enumerate() {
            let want = hex::decode(p.as_str().unwrap()).unwrap();
            if levels[k] > 0 {
                assert_eq!(r.blocks[k].as_deref(), Some(&want[..]), "{name} tile {k}");
            }
        }
        let _ = frame::NO_FEEDBACK;
    }
}

#[test]
fn legacy_minimodem_framing_matches_python() {
    use dos::legacy::{wrap, ArqPacket, Framer};
    let f = Framer { fec: 16 };
    for case in golden("legacy.json").as_array().unwrap() {
        let raw = hex::decode(case["raw"].as_str().unwrap()).unwrap();
        let framed = hex::decode(case["framed"].as_str().unwrap()).unwrap();
        assert_eq!(f.encode(&raw), framed);
        assert_eq!(wrap(&framed), hex::decode(case["wrapped"].as_str().unwrap()).unwrap());
        assert_eq!(f.decode(&framed).unwrap(), raw);
        assert!(ArqPacket::from_bytes(&raw).is_some());
    }
}
