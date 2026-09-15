# Texture replacements

`fast3d::replacement` provides an immutable, in-memory registration provider.
Renderers start with replacements disabled. Install a snapshot with
`Renderer::set_replacement_provider(Some(Arc::new(provider)))`, then call
`begin_frame`. A queued update, including removal with `None`, takes effect at
that boundary. Calls between boundaries keep using the active snapshot.

`ReplacementProvider::new(revision, registrations)` owns its registrations.
The revision is a consumer-supplied label. Installing another snapshot invalidates
all previous decisions, even if its revision label is unchanged. Reconfiguration
and device recreation clear active and queued providers and all replacement GPU
objects. Reinstall a provider on the new renderer to upload its images again.

## Registering an image

Build a `ReplacementTarget` from a `DecodeRecipe`, owned `EncodedInput`, and
`TextureKeyScheme`. The constructor validates the recipe and constructs its exact
witness without decoding pixels. `EncodedInput::Tmem` owns an `Arc<[u8; 4096]>` in
physical N64 texture-byte order. `LinearCompat` owns the reconstructed encoded
stream and its palette bank. Base and line are in eight-byte words; TLUT is the
extracted mode `0`–`3`. CI4 uses palette selectors 0–15; other formats use zero.
Non-CI recipes use TLUT zero. Native CI targets can retain modes zero and one,
whose native output is transparent black; these modes have no v5 alias.

`ReplacementTarget::key()` exposes the versioned numeric key and `witness()`
exposes the exact scheme-specific bytes. The witness cannot be supplied separately
or replaced with an arbitrary hash. A registration pairs that target with
`ReplacementStatus::Ready(ReplacementImage { extent, pixels, format })` or
`ReplacementStatus::Pending`.

Ready pixels are owned `Arc<[u8]>` RGBA8, in linear UNORM colour space, with
straight alpha. `format` must be `wgpu::TextureFormat::Rgba8Unorm`. There is no
sRGB conversion, premultiplication, mip generation, guest-memory access or file
I/O during lookup. Consumers can load data asynchronously before installing a
new snapshot.

Tiles and linear images accept their native dimensions or a positive integer
scale shared by both axes. Both dimensions must fit the device's maximum 2D
texture dimension. Scale is at most 32,767 so a mirrored 15-bit mask period fits
signed shader arithmetic. Zero dimensions, fractional or unequal axis scales, overflow,
and a length other than `width * height * 4` are rejected. A lookup image accepts
only 4096×4. Framebuffer views, dummy images and already-RGBA compatibility inputs
do not enter replacement lookup.

## Lookup and lifetime

Lookup tries an exact Fast3dV1 target first. If absent, a provider containing v5
registrations can request an eligible exact Rt64V5 alias. If both schemes have an
exact registration, Fast3dV1 wins. Within a scheme, the last exact registration
wins. Numeric equality alone never authorizes a replacement.

An exact pending or invalid Fast3dV1 registration stops the search and uses the
native image. It does not fall through to v5. A missing registration also uses
native output. Invalid images increment `replacement.invalid` and emit a warning
with the provider revision and reason. Pending and no-match outcomes have their
own counters. All outcomes, including aliases, are cached until input changes,
eviction or the next installed provider snapshot.

Decision reuse checks the exact recipe, full owned encoded input, and v5
eligibility. Checking only the native witness would be insufficient: v5 includes
six additional bytes of every used palette slot. The decision retains the
resolved target witness and recipe. Native decoded residency always uses its
original Fast3dV1 key and witness, including when replacements are enabled.

The decision cache admits at most 256 entries, 8 MiB of charged CPU payload and
64 MiB of GPU payload. CPU charges include encoded input, witness and retained
RGBA bytes per entry; sharing may make the actual allocation smaller. Because
RGBA bytes also count against the CPU cap, that cap can bind before the GPU cap.
Oversized entries bypass the cache. Eviction drops cache ownership; bound and
submitted images remain alive. Queue completion releases submission ownership.
These caps do not limit caller-owned registration snapshots or in-flight work.

Counters distinguish provider calls, witness/input comparisons, decisions,
pending/invalid/no-match results, v5 admission/hash bytes, uploads, evictions and
bypasses. Gauges report provider payload, charged cache payload, deduplicated live
RGBA CPU storage, live/pinned/transient/in-flight GPU storage and allocation
overhead. Provider-off rendering makes no provider calls, admission checks or v5
hashes. Existing native TMEM counters retain their meanings.

## Sampling scaled images

N64 tile bounds, shifts, masks, clamp/mirror flags and logical extents remain in
N64 texel units. LOD and role selection use those original dimensions. After the
ordinary tile-coordinate calculation, a replacement multiplies the coordinate
by its integer scale. Addressing uses correspondingly scaled clamp extents and
mask periods. Point filtering floors that coordinate; three-point filtering uses
its fractional part and the same triangular interpolation rule. Alpha follows
the same interpolation as RGB. No half-texel shift or mip bias is added. Same-size
and lookup replacements use the existing sampling path.

For example, a scale of two maps a native coordinate of 0.125 to replacement
coordinate 0.25. Three-point filtering at `(0.25, 0.25)` weights taps `(0,0)`,
`(1,0)`, `(0,1)` by `1/2`, `1/4`, `1/4`. A two-texel native mirror period becomes
four replacement texels, addressed `0,1,2,3,3,2,1,0`.

## Rt64V5 compatibility

The alias matches the version-5 `TMEMHasher` at rt64 commit
`43373749dac9bbc1b653e6a02aed40a9e1783bed`. It admits supported physical Tile
requests with equal logical/output extents, nonzero line, drawn row bytes no
larger than stride, and no bank crossing by a row or swapped tail. Four-bit
widths must be even. CI requires RGBA16 or IA16 TLUT; non-CI requires raw TLUT off,
checked from the retained sampling state before lookup. Lookup and linear
compatibility requests have no v5 alias.

The stream contains physical row spans, the low bank followed by the high bank
for RGBA32, and used palette slots in increasing index order. Each palette slot
contributes all eight bytes. Its little-endian tail is width u16, height u16,
shifted TLUT u32 (`0`, `0x8000`, `0xc000`), line u16, size u8, format u8.
Base and palette select bytes but are not appended. Relocation can match only
when the resolved stream and proved interpretation are exactly equal.

`rt64-v5-vectors.json` freezes 82 streams, span lists and hashes. Run the probe
against an unchanged reference checkout:

```sh
python3 tools/tmem/verify_rt64_v5.py --reference /path/to/rt64-reference --output /new/evidence/directory
cargo test -p fast3d --test rt64_v5
```

`rt64-v5-fixtures.json` also freezes IDs and streams for admitted requests in
the three committed TMEM fixtures; the residency tests check them on native and
wasm. The verifier authenticates the pinned headers and probe source before compiling.
It never regenerates expected vectors. Hash-only pack records, older rt64 versions,
filesystem/archive importers and HD sampling parity with rt64 are not implemented.
