use dos::channel::{AcousticChannel, Interferer};
use dos::frame::Header;
use dos::link::{build_frame, receive_frame};
use dos::phy::{Phy, PhyConfig};
use std::time::Instant;

fn main() {
    let phy = Phy::new(PhyConfig::default());
    let levels = vec![3u8; phy.cfg.n_tiles()];
    let t = Instant::now();
    let (wave, pl) = build_frame(&phy, &Header::new(0, levels.clone()), &levels, |n| vec![7u8; n]);
    println!("build   {:.3}s", t.elapsed().as_secs_f64());
    let mut ch = AcousticChannel::new("dell-g5_to_e540", 1).unwrap();
    let t = Instant::now();
    let rx = ch.apply(&wave, 48000.0, &[Interferer::new(2000.0, 4000.0, -3.0)]);
    println!("channel {:.3}s (len {})", t.elapsed().as_secs_f64(), rx.len());
    let t = Instant::now();
    let s = phy.find_sync(&rx);
    println!("sync    {:.3}s {:?}", t.elapsed().as_secs_f64(), s);
    let t = Instant::now();
    let r = receive_frame(&phy, &rx, None, Some(&pl));
    println!("receive {:.3}s ok {}", t.elapsed().as_secs_f64(), r.tile_ok.iter().filter(|&&o| o).count());
}
