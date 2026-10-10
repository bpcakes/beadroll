//! Object names as git computes them: the SHA-1 or SHA-256 of `"<type> <size>\0"` and the content.
//! SHA-1 is written out here (FIPS 180-4); git's collision-detecting SHA-1 gives the same digest
//! for every input that is not a collision attack, which git refuses anyway.

use sha2::{Digest as _, Sha256};

/// The name git gives an object of `kind` (`blob`, `tree`, `commit`, `tag`) holding `data`, in the
/// format whose names are `hex_len` hex digits long (40: SHA-1, 64: SHA-256); `None` for any other
/// length.
pub(crate) fn object_id(kind: &str, data: &[u8], hex_len: usize) -> Option<String> {
    let header = format!("{kind} {}\0", data.len());
    let digest: Vec<u8> = match hex_len {
        40 => sha1([header.as_bytes(), data]).to_vec(),
        64 => {
            let mut hasher = Sha256::new();
            hasher.update(header.as_bytes());
            hasher.update(data);
            hasher.finalize().to_vec()
        }
        _ => return None,
    };
    Some(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// The SHA-1 digest of `parts`, one after another. The objects bead checks are small: the message
/// is padded in a copy.
fn sha1<const N: usize>(parts: [&[u8]; N]) -> [u8; 20] {
    let mut message = parts.concat();
    let bits = (message.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bits.to_be_bytes());
    let mut state: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    for block in message.as_chunks::<64>().0 {
        compress(&mut state, block);
    }
    let mut digest = [0u8; 20];
    for (out, word) in digest.as_chunks_mut::<4>().0.iter_mut().zip(state) {
        *out = word.to_be_bytes();
    }
    digest
}

/// One 64-byte block of SHA-1.
fn compress(state: &mut [u32; 5], block: &[u8; 64]) {
    let mut schedule = [0u32; 80];
    for (word, bytes) in schedule.iter_mut().zip(block.as_chunks::<4>().0) {
        *word = u32::from_be_bytes(*bytes);
    }
    for at in 16..80 {
        schedule[at] =
            (schedule[at - 3] ^ schedule[at - 8] ^ schedule[at - 14] ^ schedule[at - 16])
                .rotate_left(1);
    }
    let [mut a, mut b, mut c, mut d, mut e] = *state;
    for (at, word) in schedule.iter().enumerate() {
        let (f, k) = match at {
            0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
            20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
            40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
            _ => (b ^ c ^ d, 0xCA62_C1D6),
        };
        let next = a
            .rotate_left(5)
            .wrapping_add(f)
            .wrapping_add(e)
            .wrapping_add(k)
            .wrapping_add(*word);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = next;
    }
    for (word, add) in state.iter_mut().zip([a, b, c, d, e]) {
        *word = word.wrapping_add(add);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn sha1_matches_the_standard_vectors() {
        assert_eq!(
            hex(&sha1([b""])),
            "da39a3ee5e6b4b0d3255bfef95601890afd80709"
        );
        assert_eq!(
            hex(&sha1([b"abc"])),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        let long = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        assert_eq!(
            hex(&sha1([long])),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
        let million = vec![b'a'; 1_000_000];
        assert_eq!(
            hex(&sha1([&million[..]])),
            "34aa973cd4c4daa4f61eeb2bdbad27316534016f"
        );
        // split anywhere, the same digest
        assert_eq!(sha1([&long[..7], &long[7..]]), sha1([long]));
    }

    #[test]
    fn object_ids_are_gits() {
        // `git hash-object --stdin <<< hello` and its SHA-256 twin
        assert_eq!(
            object_id("blob", b"hello\n", 40).unwrap(),
            "ce013625030ba8dba906f756967f9e9ca394464a"
        );
        assert_eq!(
            object_id("blob", b"hello\n", 64).unwrap(),
            "2cf8d83d9ee29543b34a87727421fdecb7e3f3a183d337639025de576db9ebb4"
        );
        // the empty tree, in both formats
        assert_eq!(
            object_id("tree", b"", 40).unwrap(),
            "4b825dc642cb6eb9a060e54bf8d69288fbee4904"
        );
        assert_eq!(
            object_id("tree", b"", 64).unwrap(),
            "6ef19b41225c5369f1c104d45d8d85efa9b057b53b14b4b9b939dd74decc5321"
        );
        assert_eq!(object_id("blob", b"", 41), None);
    }
}
