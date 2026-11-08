//! Regression tests (ported from the original Python implementation) plus a two-node exchange.

use dos::channel::AcousticChannel;
use dos::frame::Header;
use dos::link::{build_frame, receive_frame};
use dos::node::Node;
use dos::phy::{Phy, PhyConfig};
use dos::traffic::{FileReceiver, FileSender, PacketQueue, PacketReassembly, Traffic};
use rand::rngs::StdRng;
use rand::{Rng, RngCore, SeedableRng};

#[test]
fn loopback_all_levels() {
    let phy = Phy::new(PhyConfig::default());
    let n = phy.cfg.n_tiles();
    let levels: Vec<u8> = (0..n).map(|k| (k % 4) as u8).collect();
    let mut rng = StdRng::seed_from_u64(0);
    let (wave, pl) = build_frame(&phy, &Header::new(7, levels.clone()), &levels, |n| {
        let mut b = vec![0; n];
        rng.fill_bytes(&mut b);
        b
    });
    let rx = [vec![0.0; 3000], wave, vec![0.0; 3000]].concat();
    let r = receive_frame(&phy, &rx, None, Some(&pl));
    let h = r.header.unwrap();
    assert_eq!((h.seq, h.levels), (7, levels.clone()));
    assert!((0..n).all(|k| levels[k] == 0 || r.tile_ok[k]));
}

#[test]
fn selective_repeat_over_lossy_link() {
    let mut rng = StdRng::seed_from_u64(2);
    let mut data = vec![0u8; 5000];
    rng.fill_bytes(&mut data);
    let (mut tx, mut rx) = (FileSender::new(data.clone(), 3), FileReceiver::default());
    let caps: Vec<usize> = (0..40).map(|i| [10, 70, 106, 0][i % 4]).collect();
    for seq in 0..200u16 {
        let blocks = tx.fill(seq, &caps);
        let mut ok: Vec<bool> = (0..caps.len()).map(|_| rng.gen::<f64>() > 0.4).collect();
        ok[0] = false; // tile 0 always fails: must not starve anything
        for (k, b) in blocks.iter().enumerate() {
            if ok[k] && !b.is_empty() {
                rx.put(b);
            }
        }
        if rng.gen::<f64>() < 0.8 {
            tx.ack(seq, &ok);
            tx.requeue_before(seq);
        }
        if tx.done() {
            break;
        }
    }
    assert!(tx.done() && rx.done() && rx.buf == data);
}

#[test]
fn ip_packets_fragment_retransmit_reassemble() {
    let mut rng = StdRng::seed_from_u64(4);
    let pkts: Vec<Vec<u8>> = (0..40)
        .map(|_| {
            let mut p = vec![0u8; rng.gen_range(20..1200)];
            rng.fill_bytes(&mut p);
            p
        })
        .collect();
    let (mut q, mut rx) = (PacketQueue::new(5), PacketReassembly::default());
    for p in &pkts {
        q.push(p.clone());
    }
    q.push(pkts[0].clone()); // identical packet still waiting: dropped
    let caps: Vec<usize> = (0..92).map(|i| [10, 70, 106, 0][i % 4]).collect();
    let mut got = vec![];
    for seq in 0..60u16 {
        let blocks = q.fill(seq, &caps);
        let ok: Vec<bool> = (0..caps.len()).map(|_| rng.gen::<f64>() > 0.2).collect();
        for (k, b) in blocks.iter().enumerate() {
            if ok[k] && !b.is_empty() {
                got.extend(rx.put(b));
            }
        }
        q.ack(seq, &ok);
    }
    assert_eq!(q.stats.dropped_dup, 1);
    // delivered packets are intact and unique; missing ones are explained by given-up fragments
    assert!(got.iter().all(|g| pkts.contains(g)));
    let mut uniq = got.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(uniq.len(), got.len());
    assert!(pkts.len() - got.len() <= q.stats.given_up as usize);
    assert!(got.len() as f64 >= 0.85 * pkts.len() as f64);
}

#[test]
fn two_nodes_exchange_files_over_simulated_room() {
    let phy = Phy::new(PhyConfig::default());
    let (da, db) = (b"hello from A ".repeat(300), b"and B replies ".repeat(120));
    let mut a = Node::new(&phy, FileSender::new(da.clone(), 1), 1);
    let mut b = Node::new(&phy, FileSender::new(db.clone(), 2), 2);
    let (mut ra, mut rb) = (FileReceiver::default(), FileReceiver::default());
    let mut ab = AcousticChannel::new("dell-g5_to_e540", 1).unwrap();
    let mut ba = AcousticChannel::new("e540_to_dell-g5", 2).unwrap();
    for _ in 0..12 {
        let w = a.make_frame(&phy);
        b.on_receive(&phy, &ab.apply(&w, 48000.0, &[]), |blk| rb.put(blk));
        let w = b.make_frame(&phy);
        a.on_receive(&phy, &ba.apply(&w, 48000.0, &[]), |blk| ra.put(blk));
        if a.traffic.done() && b.traffic.done() {
            break;
        }
    }
    assert_eq!(rb.buf, da);
    assert_eq!(ra.buf, db);
    assert!(a.out_levels.iter().any(|&l| l >= 2), "adaptation should move off the robust map");
}
