//! Public microcode selector (spec §3.6). Mirrors the crate-internal `crate::hle::gbi::GbiUcode`.

use crate::hle::gbi::{detect_from_ucode_data, detect_from_ucode_hash, GbiUcode};

/// Which N64 graphics microcode a display list targets. An explicit per-`process_dl` argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Microcode {
    /// Authentic F3DEX2 (web / RdramImage default). PC ports (sm64/helix, wafel) that
    /// emit `GBI_FLOATS` data also select this microcode and pair it with `DataFormat::Float`.
    #[default]
    F3dex2,
    /// Original F3D microcode.
    F3d,
    /// F3DZEX2 (`F3DZEX.NoN fifo 2.08I` and relatives), used by The Legend of Zelda:
    /// Majora's Mask and Ocarina of Time. F3DEX2 plus `G_BRANCH_W`, point lights and no
    /// near-plane clipping.
    F3dzex2,
    /// S2DEX2 (`S2DEX  fifo 2.08` and relatives), the 2D background/sprite microcode paired
    /// with F3DEX2. Display lists usually switch to it with `gSPLoadUcodeL`; its object
    /// commands aren't drawn yet.
    S2dex2,
}

impl From<Microcode> for GbiUcode {
    fn from(m: Microcode) -> Self {
        match m {
            Microcode::F3dex2 => GbiUcode::F3dex2,
            Microcode::F3d => GbiUcode::F3d,
            Microcode::F3dzex2 => GbiUcode::F3dzex2,
            Microcode::S2dex2 => GbiUcode::S2dex2,
        }
    }
}

impl From<GbiUcode> for Microcode {
    fn from(u: GbiUcode) -> Self {
        match u {
            GbiUcode::F3dex2 => Microcode::F3dex2,
            GbiUcode::F3d => Microcode::F3d,
            GbiUcode::F3dzex2 => Microcode::F3dzex2,
            GbiUcode::S2dex2 => Microcode::S2dex2,
        }
    }
}

/// Identifies a microcode from its data segment (`OSTask::ucode_data`, big-endian bytes)
/// by the version string it embeds. Returns `None` for microcodes fast3d doesn't implement.
pub fn detect_microcode_from_data(ucode_data: &[u8]) -> Option<Microcode> {
    detect_from_ucode_data(ucode_data).map(Microcode::from)
}

/// Returns `None` for every hash until a ROM consumer supplies verified microcode
/// records and a defined hash algorithm. Select [`Microcode`] explicitly for ports
/// and web consumers; [`crate::DataFormat`] is a separate data-layout choice.
pub fn detect_microcode(ucode_hash: u64) -> Option<Microcode> {
    detect_from_ucode_hash(ucode_hash).map(Microcode::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_roundtrips_both_directions() {
        assert_eq!(GbiUcode::from(Microcode::F3dex2), GbiUcode::F3dex2);
        assert_eq!(GbiUcode::from(Microcode::F3d), GbiUcode::F3d);
        assert_eq!(Microcode::from(GbiUcode::F3dex2), Microcode::F3dex2);
        assert_eq!(Microcode::from(GbiUcode::F3d), Microcode::F3d);
        assert_eq!(GbiUcode::from(Microcode::F3dzex2), GbiUcode::F3dzex2);
        assert_eq!(Microcode::from(GbiUcode::F3dzex2), Microcode::F3dzex2);
        assert_eq!(Microcode::default(), Microcode::F3dex2);
    }
}
