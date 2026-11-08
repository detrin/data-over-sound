//! Fixed pseudo-random tables that define the on-air format.
//!
//! The default configuration uses fixed tables (`data/wire_v1.bin`, originally numpy PCG64 draws
//! from the first, Python, implementation; the on-air format is unchanged since). Any other
//! configuration gets tables from a deterministic SplitMix64 generator.

const WIRE_V1: &[u8] = include_bytes!("../data/wire_v1.bin");

pub struct WireTables {
    /// reference phase index per subcarrier (phase = pi/2 * k + pi/4)
    pub reference: Vec<u8>,
    /// pilot DQPSK step index, [data symbol][subcarrier]
    pub pilots: Vec<Vec<u8>>,
    /// header byte-slot permutation
    pub header_slots: Vec<usize>,
}

impl WireTables {
    pub fn new(n_bins: usize, n_data: usize, n_slots: usize) -> Self {
        Self::from_v1(n_bins, n_data, n_slots).unwrap_or_else(|| Self::generated(n_bins, n_data, n_slots))
    }

    fn from_v1(n_bins: usize, n_data: usize, n_slots: usize) -> Option<Self> {
        let b = WIRE_V1;
        if &b[..4] != b"DOS1" {
            return None;
        }
        let rd = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize;
        if (rd(4), rd(8), rd(12)) != (n_bins, n_data, n_slots) {
            return None;
        }
        let mut o = 16;
        let reference = b[o..o + n_bins].to_vec();
        o += n_bins;
        let pilots = (0..n_data).map(|s| b[o + s * n_bins..o + (s + 1) * n_bins].to_vec()).collect();
        o += n_data * n_bins;
        let header_slots = (0..n_slots).map(|i| u16::from_le_bytes([b[o + 2 * i], b[o + 2 * i + 1]]) as usize).collect();
        Some(Self { reference, pilots, header_slots })
    }

    fn generated(n_bins: usize, n_data: usize, n_slots: usize) -> Self {
        let mut rng = SplitMix64(0x0D05_0F_D0_5);
        let reference = (0..n_bins).map(|_| (rng.next() % 4) as u8).collect();
        let pilots = (0..n_data).map(|_| (0..n_bins).map(|_| (rng.next() % 4) as u8).collect()).collect();
        let mut header_slots: Vec<usize> = (0..n_slots).collect();
        for i in (1..n_slots).rev() {
            let j = (rng.next() % (i as u64 + 1)) as usize;
            header_slots.swap(i, j);
        }
        Self { reference, pilots, header_slots }
    }
}

struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}
