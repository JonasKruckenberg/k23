use crate::kconfig;
use crate::machine_info::MachineInfo;
use crate::payload::Payload;
use kmm::{Mode, VirtualAddress};
use rand_chacha::rand_core::{RngCore, SeedableRng};
use rand_chacha::ChaChaRng;

pub fn init(machine_info: &MachineInfo) -> ChaChaRng {
    let seed = &machine_info.rng_seed.expect("missing RNG seed")[0..32];

    ChaChaRng::from_seed(seed.try_into().unwrap())
}

pub fn random_offset_for_payload(rand: &mut ChaChaRng, payload: &Payload) -> VirtualAddress {
    let start = kconfig::MEMORY_MODE::PHYS_OFFSET as u64;
    let stop = u64::MAX - payload.mem_size();
    log::trace!("sampling between {start:#x}..{stop:#x}",);

    let addr = sample_single_inclusive(start, stop, rand);

    log::info!("KASLR offset {addr:#x}");
    VirtualAddress::new(addr as usize).align_down(kconfig::PAGE_SIZE)
}

#[inline]
fn sample_single_inclusive(low: u64, high: u64, rng: &mut ChaChaRng) -> u64 {
    assert!(
        low <= high,
        "UniformSampler::sample_single_inclusive: low > high"
    );
    let range = high.wrapping_sub(low).wrapping_add(1);
    // If the above resulted in wrap-around to 0, the range is $ty::MIN..=$ty::MAX,
    // and any integer will do.
    if range == 0 {
        return rng.next_u64();
    }

    let zone = if u64::MAX <= u16::MAX as u64 {
        // Using a modulus is faster than the approximation for
        // i8 and i16. I suppose we trade the cost of one
        // modulus for near-perfect branch prediction.
        let unsigned_max = u64::MAX;
        let ints_to_reject = (unsigned_max - range + 1) % range;
        unsigned_max - ints_to_reject
    } else {
        // conservative but fast approximation. `- 1` is necessary to allow the
        // same comparison without bias.
        (range << range.leading_zeros()).wrapping_sub(1)
    };

    loop {
        let v: u64 = rng.next_u64();
        let (hi, lo) = wmul(v, range);
        if lo <= zone {
            return low.wrapping_add(hi);
        }
    }
}

fn wmul(a: u64, b: u64) -> (u64, u64) {
    let tmp = (a as u128) * (b as u128);
    ((tmp >> 64) as u64, tmp as u64)
}
