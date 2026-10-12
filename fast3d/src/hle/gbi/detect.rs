//! Microcode hash lookup. Production records await a ROM consumer with a verified hash recipe.
use super::GbiUcode;

/// Identifies a microcode from its data segment (big-endian bytes, as in the ROM), using
/// the version string Nintendo's microcodes embed, e.g.
/// `RSP Gfx ucode F3DZEX.NoN fifo 2.08I Yoshitaka Yasumoto/Kawasedo 1999.`
///
/// Returns `None` for microcodes fast3d doesn't implement (F3DEX 1.x, S2DEX, L3DEX, ...).
/// `.NoN` F3DEX2 builds are reported as `F3dex2`, which near-clips; only F3DZEX2 models NoN.
pub fn detect_from_ucode_data(data: &[u8]) -> Option<GbiUcode> {
    if find(data, b"RSP SW Version: 2.0").is_some() {
        return Some(GbiUcode::F3d);
    }
    let start = find(data, b"RSP Gfx ucode ")? + b"RSP Gfx ucode ".len();
    let rest = &data[start..];
    let end = rest.iter().position(|&b| b < 0x20).unwrap_or(rest.len());
    let version = &rest[..end];
    let name_end = version
        .iter()
        .position(|&b| b == b' ' || b == b'.')
        .unwrap_or(version.len());
    let name = &version[..name_end];
    // F3DEX2-generation microcodes still call themselves F3DEX/F3DZEX, with a 2.xx version.
    let gen2 = find(&version[name_end..], b" 2.").is_some();
    match name {
        b"F3DZEX" if gen2 => Some(GbiUcode::F3dzex2),
        b"F3DEX" if gen2 => Some(GbiUcode::F3dex2),
        _ => None,
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Returns `None` until verified microcode hashes are available.
pub fn detect_from_ucode_hash(hash: u64) -> Option<GbiUcode> {
    lookup_hash(hash, &[])
}

fn lookup_hash(hash: u64, records: &[(u64, GbiUcode)]) -> Option<GbiUcode> {
    records.iter().find(|(h, _)| *h == hash).map(|(_, u)| *u)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE_RECORDS: &[(u64, GbiUcode)] = &[
        (0xF3D2_0000_0000_0001, GbiUcode::F3dex2),
        (0xF3D0_0000_0000_0003, GbiUcode::F3d),
    ];

    #[test]
    fn fixture_hashes_resolve_in_test_table() {
        assert_eq!(
            lookup_hash(0xF3D2_0000_0000_0001, FIXTURE_RECORDS),
            Some(GbiUcode::F3dex2)
        );
        assert_eq!(
            lookup_hash(0xF3D0_0000_0000_0003, FIXTURE_RECORDS),
            Some(GbiUcode::F3d)
        );
    }

    #[test]
    fn version_strings_identify_microcodes() {
        let with = |text: &str| {
            let mut data = vec![0u8; 0x40];
            data.extend_from_slice(text.as_bytes());
            data.extend_from_slice(&[0x0A, 0, 0, 0]);
            detect_from_ucode_data(&data)
        };
        // The strings in Majora's Mask (US).
        assert_eq!(
            with("RSP Gfx ucode F3DZEX.NoN fifo 2.08I Yoshitaka Yasumoto/Kawasedo 1999."),
            Some(GbiUcode::F3dzex2)
        );
        assert_eq!(
            with("RSP Gfx ucode S2DEX  fifo 2.08  Yoshitaka Yasumoto 1999."),
            None
        );
        assert_eq!(
            with("RSP Gfx ucode F3DEX       fifo 2.08  Yoshitaka Yasumoto 1999."),
            Some(GbiUcode::F3dex2)
        );
        assert_eq!(
            with("RSP Gfx ucode F3DEX.NoN   fifo 2.05  Yoshitaka Yasumoto 1998."),
            Some(GbiUcode::F3dex2)
        );
        // F3DEX 1.x isn't implemented.
        assert_eq!(
            with("RSP Gfx ucode F3DEX       1.23 Yoshitaka Yasumoto 1997."),
            None
        );
        assert_eq!(with("RSP SW Version: 2.0D, 04-01-96"), Some(GbiUcode::F3d));
        assert_eq!(with("no version here"), None);
    }

    #[test]
    fn unknown_hash_is_none() {
        assert_eq!(lookup_hash(0xDEAD_BEEF, FIXTURE_RECORDS), None);
    }
}
