//! CPython's `random.Random`, as far as the models draw from it: the Mersenne Twister (MT19937) of
//! `Modules/_randommodule.c`, seeded from an integer as `random.seed` seeds it, and `randrange`,
//! `choice` and weighted `choices` built on it as `Lib/random.py` builds them (unchanged from
//! Python 3.11 to 3.14). A seed therefore makes the very choices Python's model made with it.

/// The words of the generator's state, and the offset of its second tap.
const N: usize = 624;
const M: usize = 397;

/// The generator: its state, and the next word to temper.
pub(crate) struct Rng {
    state: [u32; N],
    index: usize,
}

impl Rng {
    /// `random.Random(seed)`: `init_by_array` over the seed's 32-bit words, least significant
    /// first (`random_seed`).
    pub(crate) fn new(seed: u64) -> Rng {
        let (low, high) = ((seed & 0xffff_ffff) as u32, (seed >> 32) as u32);
        let key: &[u32] = if high == 0 { &[low] } else { &[low, high] };
        let mut rng = Rng {
            state: [0; N],
            index: N,
        };
        rng.init_by_array(key);
        rng
    }

    /// `init_genrand(s)`.
    fn init_genrand(&mut self, s: u32) {
        self.state[0] = s;
        for i in 1..N {
            let prev = self.state[i - 1];
            self.state[i] = 1_812_433_253_u32
                .wrapping_mul(prev ^ (prev >> 30))
                .wrapping_add(i as u32);
        }
        self.index = N;
    }

    /// `init_by_array(key)`.
    fn init_by_array(&mut self, key: &[u32]) {
        self.init_genrand(19_650_218);
        let mt = &mut self.state;
        let (mut i, mut j) = (1, 0);
        for _ in 0..N.max(key.len()) {
            let prev = mt[i - 1];
            mt[i] = (mt[i] ^ (prev ^ (prev >> 30)).wrapping_mul(1_664_525))
                .wrapping_add(key[j])
                .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= N {
                mt[0] = mt[N - 1];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
        }
        for _ in 1..N {
            let prev = mt[i - 1];
            mt[i] =
                (mt[i] ^ (prev ^ (prev >> 30)).wrapping_mul(1_566_083_941)).wrapping_sub(i as u32);
            i += 1;
            if i >= N {
                mt[0] = mt[N - 1];
                i = 1;
            }
        }
        mt[0] = 0x8000_0000;
    }

    /// `genrand_uint32`: the next 32 random bits, the state regenerated every 624 words.
    fn next_u32(&mut self) -> u32 {
        if self.index >= N {
            for k in 0..N {
                let y = (self.state[k] & 0x8000_0000) | (self.state[(k + 1) % N] & 0x7fff_ffff);
                let odd = if y & 1 == 1 { 0x9908_b0df } else { 0 };
                self.state[k] = self.state[(k + M) % N] ^ (y >> 1) ^ odd;
            }
            self.index = 0;
        }
        let mut y = self.state[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^ (y >> 18)
    }

    /// `getrandbits(k)` for `k` from 1 to 64: whole words, least significant first, the last one
    /// cut to its top bits.
    fn getrandbits(&mut self, k: u32) -> u64 {
        if k <= 32 {
            return u64::from(self.next_u32() >> (32 - k));
        }
        let low = u64::from(self.next_u32());
        low | (u64::from(self.next_u32() >> (64 - k)) << 32)
    }

    /// `random()`: 53 random bits as a float in [0, 1).
    fn random(&mut self) -> f64 {
        let a = f64::from(self.next_u32() >> 5);
        let b = f64::from(self.next_u32() >> 6);
        (a * 67_108_864.0 + b) * (1.0 / 9_007_199_254_740_992.0)
    }

    /// `randrange(n)` (`_randbelow`): an integer in `0..n`, drawn as `n.bit_length()` random bits
    /// until one is below `n`.
    pub(crate) fn below(&mut self, n: usize) -> usize {
        assert!(n > 0, "a choice among nothing");
        let n = n as u64;
        let bits = u64::BITS - n.leading_zeros();
        loop {
            let draw = self.getrandbits(bits);
            if draw < n {
                return draw as usize;
            }
        }
    }

    /// `randrange(start, stop)`.
    pub(crate) fn range(&mut self, start: usize, stop: usize) -> usize {
        start + self.below(stop - start)
    }

    /// `choice(items)`.
    pub(crate) fn choice<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    /// `choices(items, weights)[0]`: `random() * total` placed among the cumulative weights by
    /// `bisect_right`, whose search stops short of the last one (`hi = n - 1`).
    pub(crate) fn weighted<T: Copy>(&mut self, items: &[(T, usize)]) -> T {
        let cumulative: Vec<f64> = (items.iter())
            .scan(0, |sum, &(_, weight)| {
                *sum += weight;
                Some(*sum as f64)
            })
            .collect();
        let point = self.random() * cumulative[cumulative.len() - 1];
        let (mut lo, mut hi) = (0, items.len() - 1);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if point < cumulative[mid] {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        items[lo].0
    }
}

/// The 32-bit words of a seed's stream, as `getrandbits(32)` gives them in CPython, for a seed of
/// one word, of none (0) and of two.
#[test]
fn words_are_cpythons() {
    let words = |seed, n| {
        let mut rng = Rng::new(seed);
        (0..n).map(|_| rng.getrandbits(32)).collect::<Vec<_>>()
    };
    let first = [
        2_125_154_140,
        349_270_860,
        2_894_928_510,
        1_230_876_705,
        3_939_206_928,
    ];
    assert_eq!(words(20_261_006, 5), first);
    assert_eq!(words(0, 3), [3_626_764_237, 1_654_615_998, 3_255_389_356]);
    let two_words = (1 << 40) + 12_345;
    assert_eq!(
        words(two_words, 3),
        [1_332_995_613, 1_500_173_691, 493_462_063]
    );
}

/// `random()`, `getrandbits` past 32 bits and `randrange` past them, as CPython draws them.
#[test]
fn floats_and_wide_draws_are_cpythons() {
    let mut rng = Rng::new(0);
    for _ in 0..3 {
        rng.next_u32();
    }
    assert_eq!(rng.random(), 0.890_243_920_837_131);
    let mut rng = Rng::new(20_261_006);
    assert_eq!(rng.getrandbits(33), 2_125_154_140);
    assert_eq!(rng.getrandbits(64), 5_286_575_196_278_168_190);
    assert_eq!(rng.below(1_000_000_000_000), 317_471_819_536);
}

/// The draws the models make, in turn, as CPython makes them from a worker's seed: a weighted
/// action, a choice among 36, `randrange(5)`, `randrange(40, 100)` and a choice among one.
#[test]
fn model_draws_are_cpythons() {
    let weights = [28, 8, 6, 14, 6, 5, 5, 5, 8, 5, 4, 6];
    let actions: Vec<(usize, usize)> = weights.into_iter().enumerate().collect();
    let keys: Vec<usize> = (0..36).collect();
    let mut rng = Rng::new(20_261_007 + 3 * 101 + 2 * 907);
    let mut drawn = Vec::new();
    for _ in 0..6 {
        let action = rng.weighted(&actions);
        let key = *rng.choice(&keys);
        let (priority, until) = (rng.below(5), rng.range(40, 100));
        let only = *rng.choice(&[0]);
        drawn.push((action, key, priority, until, only));
    }
    let python = [
        (4, 8, 0, 40, 0),
        (0, 3, 3, 68, 0),
        (3, 32, 3, 91, 0),
        (0, 12, 4, 88, 0),
        (8, 17, 2, 65, 0),
        (10, 5, 2, 78, 0),
    ];
    assert_eq!(drawn, python);
    assert_eq!(
        (rng.random(), rng.random()),
        (0.649_092_110_712_723_8, 0.583_485_413_549_663)
    );
}
