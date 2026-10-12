//! RT64's extended GBI (`rt64_extended_gbi.h`), which recompiled games' patches use for
//! interpolation, widescreen and state push/pop.
//!
//! A display list enables it with a hook on `G_SPNOOP` (`gEXEnable`), choosing an otherwise
//! unused opcode (usually `0x64`); each extended command then carries a subcommand in `w0`'s
//! low 24 bits and occupies one to three command slots.
//!
//! Interpolation (matrix groups), widescreen alignment, refresh rate and upscaling hints have
//! no effect on a single 4:3 frame and are accepted as no-ops. Commands that draw or change
//! render state are implemented; the vertex Z test (`gEXVertexZTest`, used for occlusion
//! tests such as lens flares) needs a depth readback and drops the geometry it guards.

use crate::hle::math::Mat4;
use crate::hle::rsp::{Rsp, Scissor};

/// `w0` low 24 bits of the hook command: "RT" followed by 0x64.
pub(crate) const HOOK_MAGIC: u32 = 0x52_5464;

/// Hook operations (`w1 >> 28`).
pub(crate) mod hook {
    pub const GET_VERSION: u32 = 0x0;
    pub const ENABLE: u32 = 0x1;
    pub const DISABLE: u32 = 0x2;
    pub const DL: u32 = 0x3;
    pub const BRANCH: u32 = 0x4;
}

/// Extended subcommands (`w0 & 0xFFFFFF`), `G_EX_*_V1`.
pub(crate) mod op {
    pub const NOOP: u32 = 0x00;
    pub const PRINT: u32 = 0x01;
    pub const TEXRECT: u32 = 0x02;
    pub const FILLRECT: u32 = 0x03;
    pub const SETVIEWPORT: u32 = 0x04;
    pub const SETSCISSOR: u32 = 0x05;
    pub const SETRECTALIGN: u32 = 0x06;
    pub const SETVIEWPORTALIGN: u32 = 0x07;
    pub const SETSCISSORALIGN: u32 = 0x08;
    pub const SETREFRESHRATE: u32 = 0x09;
    pub const VERTEXZTEST: u32 = 0x0A;
    pub const ENDVERTEXZTEST: u32 = 0x0B;
    pub const MATRIXGROUP: u32 = 0x0C;
    pub const POPMATRIXGROUP: u32 = 0x0D;
    pub const FORCEUPSCALE2D: u32 = 0x0E;
    pub const FORCETRUEBILERP: u32 = 0x0F;
    pub const FORCESCALELOD: u32 = 0x10;
    pub const FORCEBRANCH: u32 = 0x11;
    pub const SETRENDERTORAM: u32 = 0x12;
    pub const EDITGROUPBYADDRESS: u32 = 0x13;
    pub const VERTEX: u32 = 0x14;
    pub const PUSHVIEWPORT: u32 = 0x15;
    pub const POPVIEWPORT: u32 = 0x16;
    pub const PUSHSCISSOR: u32 = 0x17;
    pub const POPSCISSOR: u32 = 0x18;
    pub const PUSHOTHERMODE: u32 = 0x19;
    pub const POPOTHERMODE: u32 = 0x1A;
    pub const PUSHCOMBINE: u32 = 0x1B;
    pub const POPCOMBINE: u32 = 0x1C;
    pub const PUSHPROJMATRIX: u32 = 0x1D;
    pub const POPPROJMATRIX: u32 = 0x1E;
    pub const PUSHENVCOLOR: u32 = 0x1F;
    pub const POPENVCOLOR: u32 = 0x20;
    pub const PUSHBLENDCOLOR: u32 = 0x21;
    pub const POPBLENDCOLOR: u32 = 0x22;
    pub const PUSHFOGCOLOR: u32 = 0x23;
    pub const POPFOGCOLOR: u32 = 0x24;
    pub const PUSHFILLCOLOR: u32 = 0x25;
    pub const POPFILLCOLOR: u32 = 0x26;
    pub const PUSHPRIMCOLOR: u32 = 0x27;
    pub const POPPRIMCOLOR: u32 = 0x28;
    pub const PUSHGEOMETRYMODE: u32 = 0x29;
    pub const POPGEOMETRYMODE: u32 = 0x2A;
    pub const SETDITHERNOISESTRENGTH: u32 = 0x2B;
    pub const SETRDRAMEXTENDED: u32 = 0x2C;
    pub const SETNEARCLIPPING: u32 = 0x2D;
    /// One past the last known subcommand.
    #[cfg(test)]
    pub const MAX: u32 = 0x2E;
}

/// How many 8-byte command slots a subcommand occupies, including the first.
pub(crate) fn command_slots(sub: u32) -> usize {
    match sub {
        op::TEXRECT | op::SETSCISSORALIGN => 3,
        op::FILLRECT
        | op::SETVIEWPORT
        | op::SETSCISSOR
        | op::SETRECTALIGN
        | op::SETVIEWPORTALIGN
        | op::MATRIXGROUP
        | op::EDITGROUPBYADDRESS
        | op::VERTEX => 2,
        _ => 1,
    }
}

/// RT64 limits each push/pop stack to 16 entries (`RDP_EXTENDED_STACK_SIZE`).
const STACK_LIMIT: usize = 16;

/// A push/pop stack that ignores pushes past the limit and pops past the bottom, like RT64.
#[derive(Clone, Debug)]
struct Stack<T>(Vec<T>);

impl<T> Default for Stack<T> {
    fn default() -> Self {
        Stack(Vec::new())
    }
}

impl<T> Stack<T> {
    fn push(&mut self, value: T) {
        if self.0.len() + 1 < STACK_LIMIT {
            self.0.push(value);
        }
    }

    fn pop(&mut self) -> Option<T> {
        self.0.pop()
    }
}

/// The primitive color state saved by `gEXPushPrimColor`.
type PrimState = ([u8; 4], f32, f32);

/// Extended GBI state for one walk.
#[derive(Debug, Default)]
pub(crate) struct ExtendedState {
    /// The opcode chosen by `gEXEnable`, or `None` while disabled.
    pub opcode: Option<u8>,
    /// Inside a `gEXVertexZTest` .. `gEXEndVertexZTest` span.
    pub z_test: bool,
    viewport: Stack<([f32; 3], [f32; 3])>,
    scissor: Stack<(Scissor, bool)>,
    other_mode: Stack<(u32, u32)>,
    combine: Stack<(u32, u32)>,
    projection: Stack<Mat4>,
    env: Stack<[u8; 4]>,
    blend: Stack<[u8; 4]>,
    fog: Stack<[u8; 4]>,
    fill: Stack<u32>,
    prim: Stack<PrimState>,
    geometry: Stack<u32>,
}

impl ExtendedState {
    /// Runs a one-slot state command (push/pop, flags). Returns `false` for subcommands that
    /// aren't state commands.
    pub(crate) fn state_command(
        &mut self,
        sub: u32,
        w1: u32,
        rsp: &mut Rsp,
        rdp: &mut crate::hle::rdp::Rdp,
    ) -> bool {
        match sub {
            op::PUSHVIEWPORT => self.viewport.push(rsp.viewport()),
            op::POPVIEWPORT => {
                if let Some(saved) = self.viewport.pop() {
                    rsp.restore_viewport(saved);
                }
            }
            op::PUSHSCISSOR => self.scissor.push((rdp.scissor, rdp.scissor_set)),
            op::POPSCISSOR => {
                if let Some((scissor, set)) = self.scissor.pop() {
                    rdp.scissor = scissor;
                    rdp.scissor_set = set;
                }
            }
            op::PUSHOTHERMODE => self.other_mode.push((rdp.other_mode_h, rdp.other_mode_l)),
            op::POPOTHERMODE => {
                if let Some((h, l)) = self.other_mode.pop() {
                    rdp.other_mode_h = h;
                    rdp.other_mode_l = l;
                    rsp.material_dirty = true;
                }
            }
            op::PUSHCOMBINE => self.combine.push((rdp.combine_l, rdp.combine_h)),
            op::POPCOMBINE => {
                if let Some((l, h)) = self.combine.pop() {
                    rdp.combine_l = l;
                    rdp.combine_h = h;
                    rsp.material_dirty = true;
                }
            }
            op::PUSHPROJMATRIX => self.projection.push(rsp.projection()),
            op::POPPROJMATRIX => {
                if let Some(saved) = self.projection.pop() {
                    rsp.restore_projection(saved);
                }
            }
            op::PUSHENVCOLOR => self.env.push(rdp.env),
            op::POPENVCOLOR => {
                if let Some(env) = self.env.pop() {
                    rdp.env = env;
                    rsp.material_dirty = true;
                }
            }
            op::PUSHBLENDCOLOR => self.blend.push(rdp.blend_color),
            op::POPBLENDCOLOR => {
                if let Some(blend) = self.blend.pop() {
                    rdp.blend_color = blend;
                    rsp.material_dirty = true;
                }
            }
            op::PUSHFOGCOLOR => self.fog.push(rdp.fog_color),
            op::POPFOGCOLOR => {
                if let Some(fog) = self.fog.pop() {
                    rdp.fog_color = fog;
                }
            }
            op::PUSHFILLCOLOR => self.fill.push(rdp.fill_color_raw),
            op::POPFILLCOLOR => {
                if let Some(fill) = self.fill.pop() {
                    rdp.fill_color_raw = fill;
                }
            }
            op::PUSHPRIMCOLOR => self
                .prim
                .push((rdp.prim, rdp.prim_lod_frac, rdp.prim_min_level)),
            op::POPPRIMCOLOR => {
                if let Some((prim, lod_frac, min_level)) = self.prim.pop() {
                    rdp.prim = prim;
                    rdp.prim_lod_frac = lod_frac;
                    rdp.prim_min_level = min_level;
                    rsp.material_dirty = true;
                }
            }
            op::PUSHGEOMETRYMODE => self.geometry.push(rsp.geometry_mode()),
            op::POPGEOMETRYMODE => {
                if let Some(mode) = self.geometry.pop() {
                    rsp.modify_geometry_mode(0, mode);
                }
            }
            op::SETNEARCLIPPING => rsp.no_near_clip = w1 & 1 == 0,
            op::FORCEBRANCH => rsp.force_branch = w1 & 1 != 0,
            // Interpolation, widescreen alignment and upscaling hints don't change a single
            // 4:3 frame.
            op::NOOP
            | op::PRINT
            | op::SETREFRESHRATE
            | op::POPMATRIXGROUP
            | op::FORCEUPSCALE2D
            | op::FORCETRUEBILERP
            | op::FORCESCALELOD
            | op::SETRENDERTORAM
            | op::SETDITHERNOISESTRENGTH
            | op::SETRDRAMEXTENDED => {}
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_counts_match_rt64_handlers() {
        assert_eq!(command_slots(op::TEXRECT), 3);
        assert_eq!(command_slots(op::SETSCISSORALIGN), 3);
        for sub in [
            op::FILLRECT,
            op::SETVIEWPORT,
            op::SETSCISSOR,
            op::SETRECTALIGN,
            op::SETVIEWPORTALIGN,
            op::MATRIXGROUP,
            op::EDITGROUPBYADDRESS,
            op::VERTEX,
        ] {
            assert_eq!(command_slots(sub), 2, "0x{sub:02X}");
        }
        for sub in (0..op::MAX).filter(|&s| {
            ![
                op::TEXRECT,
                op::SETSCISSORALIGN,
                op::FILLRECT,
                op::SETVIEWPORT,
                op::SETSCISSOR,
                op::SETRECTALIGN,
                op::SETVIEWPORTALIGN,
                op::MATRIXGROUP,
                op::EDITGROUPBYADDRESS,
                op::VERTEX,
            ]
            .contains(&s)
        }) {
            assert_eq!(command_slots(sub), 1, "0x{sub:02X}");
        }
    }

    #[test]
    fn stacks_ignore_overflow_and_underflow() {
        let mut stack = Stack::default();
        assert_eq!(stack.pop(), None::<u32>);
        for i in 0..40 {
            stack.push(i);
        }
        assert_eq!(stack.0.len(), STACK_LIMIT - 1);
        assert_eq!(stack.pop(), Some(14));
    }
}
