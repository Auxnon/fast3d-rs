// depth_read.wgsl — the depth-reading pass, shared by decal.wgsl and decal_dual.wgsl. That pass
// has no depth attachment: draws that read depth sample the scene depth written before them
// (`@group(2)`) and do their depth tests in-shader. Two kinds of draw run there:
//   - ZMODE_DEC decals: occlusion, then a coplanar bind so a decal lies ON the stored surface
//     without z-fighting (an NDC-space epsilon, NOT the N64's hardware-encoded bands [IMP11]);
//   - triangles inside a gEXVertexZTest span: drawn only if the test vertex is in front of the
//     scene depth at its own pixel, as RT64 decides it, and then depth-compared (Z_CMP) like any
//     triangle.

@group(2) @binding(0) var scene_depth: texture_depth_2d;

// The RSP-process kernel's output (rsp_process.wgsl OutVertex), for the test vertex.
struct TestVertex { pos: vec4<f32>, color: vec4<f32>, uv: vec2<f32>, no_near_clip: f32, _pad: f32 };
@group(2) @binding(1) var<storage, read> test_vertices: array<TestVertex>;

// RT64's RSPVertexTestZCS: the vertex passes when the scene depth at its pixel is farther than
// it. A vertex behind the camera or off screen fails.
fn vertex_passes_z_test(index: u32) -> bool {
    let p = test_vertices[index].pos;
    if p.w <= 0.0 {
        return false;
    }
    let ndc = p.xyz / p.w;
    let size = vec2<f32>(textureDimensions(scene_depth));
    let pixel = vec2<f32>((ndc.x * 0.5 + 0.5) * size.x, (0.5 - ndc.y * 0.5) * size.y);
    if any(pixel < vec2<f32>(0.0)) || any(pixel >= size) {
        return false;
    }
    return clamp(ndc.z, 0.0, 1.0) < textureLoad(scene_depth, vec2<i32>(pixel), 0);
}

fn depth_read_tests(in: VsOut) {
    let sampled = textureLoad(scene_depth, vec2<i32>(in.clip_position.xy), 0);
    let pixel_z = in.clip_position.z;
    let eps = 1.0e-4;
    if combiner.z_test_vertex != 0u {
        if !vertex_passes_z_test(combiner.z_test_vertex - 1u) {
            discard;
        }
        if combiner.z_test_compare != 0u && pixel_z > sampled + eps {
            discard;
        }
        return;
    }
    // Occlusion (Z_CMP): a strictly-nearer surface in the depth buffer hides the decal.
    if (pixel_z > sampled + eps) {
        discard;
    }
    // Coplanar bind: discard fragments NOT coplanar with the stored surface (per-pixel slope
    // tolerance dz absorbs depth gradient on tilted surfaces; floored at eps for flat ones).
    let dz = abs(dpdx(pixel_z)) + abs(dpdy(pixel_z));
    if (abs(pixel_z - sampled) > max(dz, eps)) {
        discard;
    }
}
