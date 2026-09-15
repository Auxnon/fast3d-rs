use super::*;
use fast3d::replacement::{
    DecodeRecipe, EncodedInput, ReplacementImage, ReplacementProvider, ReplacementRegistration,
    ReplacementStatus, ReplacementTarget, Representation as DecodeRepresentation, TextureKeyScheme,
};
use std::sync::Arc;

#[cfg(not(target_arch = "wasm32"))]
#[path = "../../../tools/readbacks/export.rs"]
mod export;

fn target(
    request: &Request,
    scheme: TextureKeyScheme,
) -> Result<ReplacementTarget, fast3d::replacement::ReplacementError> {
    let t = &request.tile;
    let representation = match request.representation {
        Representation::Tile => DecodeRepresentation::Tile,
        Representation::Lookup => DecodeRepresentation::Lookup4096x4,
        Representation::Linear => DecodeRepresentation::LinearCompat,
    };
    let recipe = DecodeRecipe {
        representation,
        output: request.extent,
        logical: [t.width.into(), t.height.into()],
        fmt: t.fmt,
        siz: t.siz,
        base: t.tmem_addr,
        line: t.line,
        palette: if t.fmt == 2 && t.siz == 0 {
            t.palette
        } else {
            0
        },
        tlut: if t.fmt == 2 { request.tlut } else { 0 },
    };
    let bank = Arc::new(request.bank.bytes.as_slice().try_into().unwrap());
    let input = if representation == DecodeRepresentation::LinearCompat {
        EncodedInput::LinearCompat {
            bytes: request.prepare().unwrap().into(),
            palette_bank: bank,
        }
    } else {
        EncodedInput::Tmem(bank)
    };
    ReplacementTarget::new(recipe, input, scheme)
}

fn ready(target: ReplacementTarget, extent: [u32; 2], pixels: Vec<u8>) -> ReplacementRegistration {
    ReplacementRegistration {
        target,
        status: ReplacementStatus::Ready(ReplacementImage {
            extent,
            pixels: pixels.into(),
            format: wgpu::TextureFormat::Rgba8Unorm,
        }),
    }
}

fn identity_provider(fixture: &Fixture, scheme: TextureKeyScheme) -> Arc<ReplacementProvider> {
    Arc::new(ReplacementProvider::new(
        1,
        requests(fixture).iter().filter_map(|request| {
            target(request, scheme).ok().map(|target| {
                ready(
                    target,
                    request.extent,
                    request
                        .decode_prepared(&request.prepare().unwrap())
                        .unwrap(),
                )
            })
        }),
    ))
}

fn expected_color(color: [u8; 3], width: usize, height: usize) -> Vec<u8> {
    let mut pixels = [0, 0, 0, 255].repeat(320 * 240);
    for y in 24..24 + height {
        for x in 16..16 + width {
            pixels[(y * 320 + x) * 4..(y * 320 + x) * 4 + 4]
                .copy_from_slice(&[color[0], color[1], color[2], 255]);
        }
    }
    pixels
}

fn export_case(
    id: &str,
    fixture: &Fixture,
    harness: &Harness,
    pixels: &[u8],
    expected: &[u8],
    before: &[u8],
) {
    assert_eq!(pixels, expected, "{id}");
    #[cfg(not(target_arch = "wasm32"))]
    {
        export::write_row(
            "FAST3D_REPLACEMENT_OUTPUT",
            id,
            "final",
            "replacement",
            "none",
            pixels,
            320,
            240,
            &fixture.to_bytes().unwrap(),
            Some(harness.renderer.device()),
        );
        if let Some(directory) = std::env::var_os("FAST3D_REPLACEMENT_OUTPUT") {
            let directory = std::path::PathBuf::from(directory);
            for (suffix, bytes) in [("expected.bin", expected), ("before.bin", before)] {
                std::fs::write(directory.join(format!("{id}.{suffix}")), bytes).unwrap();
            }
            let mask: Vec<_> = expected
                .as_chunks::<4>()
                .0
                .iter()
                .zip(before.as_chunks::<4>().0)
                .map(|(a, b)| u8::from(a != b))
                .collect();
            std::fs::write(directory.join(format!("{id}.mask.bin")), mask).unwrap();
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (fixture, harness, before);
}

async fn lifecycle() {
    let fixture = recipe_fixture(RecipeCase::LinearI8, false);
    let native = expected_recipe_pixels(RecipeCase::LinearI8, false);
    let blue = expected_color([8, 40, 232], 3, 3);
    let red = expected_color([224, 16, 32], 3, 3);
    let request = requests(&fixture).remove(0);
    let target = target(&request, TextureKeyScheme::Fast3dV1).unwrap();
    let mut harness = Harness::new(&fixture).await;
    let scope = harness
        .renderer
        .device()
        .push_error_scope(wgpu::ErrorFilter::Validation);
    let providers = [
        ("replacement-off", None, &native),
        (
            "replacement-empty",
            Some(Arc::new(ReplacementProvider::new(1, []))),
            &native,
        ),
        (
            "replacement-pending",
            Some(Arc::new(ReplacementProvider::new(
                2,
                [ReplacementRegistration {
                    target: target.clone(),
                    status: ReplacementStatus::Pending,
                }],
            ))),
            &native,
        ),
        (
            "replacement-identity",
            Some(identity_provider(&fixture, TextureKeyScheme::Fast3dV1)),
            &native,
        ),
        (
            "replacement-blue",
            Some(Arc::new(ReplacementProvider::new(
                3,
                [ready(target.clone(), [3, 3], [8, 40, 232, 71].repeat(9))],
            ))),
            &blue,
        ),
        (
            "replacement-red-same-revision",
            Some(Arc::new(ReplacementProvider::new(
                3,
                [ready(target.clone(), [3, 3], [224, 16, 32, 137].repeat(9))],
            ))),
            &red,
        ),
        ("replacement-removed", None, &native),
        (
            "replacement-invalid-extent",
            Some(Arc::new(ReplacementProvider::new(
                4,
                [ready(target.clone(), [6, 5], [0, 0, 0, 0].repeat(30))],
            ))),
            &native,
        ),
        (
            "replacement-invalid-format",
            Some(Arc::new(ReplacementProvider::new(
                5,
                [ReplacementRegistration {
                    target: target.clone(),
                    status: ReplacementStatus::Ready(ReplacementImage {
                        extent: [3, 3],
                        pixels: [0, 0, 0, 0].repeat(9).into(),
                        format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    }),
                }],
            ))),
            &native,
        ),
    ];
    for (id, provider, expected) in providers {
        harness.renderer.set_replacement_provider(provider);
        harness.renderer.begin_frame();
        harness.process(&fixture, None);
        let counts = harness.recorder.drain();
        #[cfg(not(target_arch = "wasm32"))]
        export_snapshot(id, &counts);
        assert_eq!(count(&counts, "replacement.v5_hashes"), 0);
        let pixels = harness.pixels().await;
        export_case(id, &fixture, &harness, &pixels, expected, &native);
        harness.recorder.drain();
        harness.renderer.begin_frame();
        harness.process(&fixture, None);
        let warm = harness.recorder.drain();
        assert_eq!(count(&warm, "replacement.provider_calls"), 0, "{id}");
        assert_eq!(count(&warm, "replacement.upload_calls"), 0, "{id}");
        assert_eq!(count(&warm, "replacement.v5_hashes"), 0, "{id}");
        assert_eq!(harness.pixels().await, *expected);
        harness.recorder.drain();
    }
    harness
        .renderer
        .set_replacement_provider(Some(Arc::new(ReplacementProvider::new(
            6,
            [ready(target, [3, 3], [8, 40, 232, 71].repeat(9))],
        ))));
    harness.renderer.begin_frame();
    harness.process(&fixture, None);
    harness.renderer.set_replacement_provider(None);
    harness.process(&fixture, None);
    let pixels = harness.pixels().await;
    export_case(
        "replacement-frame-frozen",
        &fixture,
        &harness,
        &pixels,
        &blue,
        &native,
    );
    harness.renderer.reconfigure(fixture.frame.config);
    harness.renderer.begin_frame();
    harness.process(&fixture, None);
    let pixels = harness.pixels().await;
    export_case(
        "replacement-new-epoch",
        &fixture,
        &harness,
        &pixels,
        &native,
        &native,
    );
    assert!(scope.pop().await.is_none());
}

async fn identity_roles_and_aliases() {
    for (name, scheme, id) in [
        (
            "tmem-layouts",
            TextureKeyScheme::Fast3dV1,
            "replacement-formats",
        ),
        (
            "tmem-layouts",
            TextureKeyScheme::Rt64V5,
            "replacement-v5-formats",
        ),
        (
            "tmem-roles-lifetime",
            TextureKeyScheme::Fast3dV1,
            "replacement-roles",
        ),
        (
            "tmem-tlut-mutation",
            TextureKeyScheme::Fast3dV1,
            "replacement-palette-banks",
        ),
    ] {
        let fixture = fixture(name);
        let mut harness = Harness::new(&fixture).await;
        harness
            .renderer
            .set_replacement_provider(Some(identity_provider(&fixture, scheme)));
        harness.renderer.begin_frame();
        harness.process(&fixture, None);
        let counts = harness.recorder.drain();
        #[cfg(not(target_arch = "wasm32"))]
        export_snapshot(id, &counts);
        assert!(count(&counts, "replacement.matches") > 0);
        assert_eq!(
            count(&counts, "tmem.decode_dispatches"),
            0,
            "{id}: {:?}",
            counts.counters
        );
        assert_eq!(
            count(&counts, "replacement.v5_hashes") > 0,
            scheme == TextureKeyScheme::Rt64V5
        );
        let expected = semantics::expected_pixels(name);
        let pixels = harness.pixels().await;
        export_case(id, &fixture, &harness, &pixels, &expected, &expected);
        harness.recorder.drain();
        harness.renderer.begin_frame();
        harness.process(&fixture, None);
        let counts = harness.recorder.drain();
        #[cfg(not(target_arch = "wasm32"))]
        export_snapshot(&format!("{id}-warm"), &counts);
        assert_eq!(count(&counts, "replacement.provider_calls"), 0);
        assert_eq!(count(&counts, "replacement.v5_hashes"), 0);
        assert_eq!(count(&counts, "replacement.upload_calls"), 0);
    }
    for recipe in RecipeCase::ALL {
        let fixture = recipe_fixture(recipe, false);
        let mut harness = Harness::new(&fixture).await;
        harness
            .renderer
            .set_replacement_provider(Some(identity_provider(
                &fixture,
                TextureKeyScheme::Fast3dV1,
            )));
        harness.renderer.begin_frame();
        harness.process(&fixture, None);
        let expected = expected_recipe_pixels(recipe, false);
        let pixels = harness.pixels().await;
        export_case(
            match recipe {
                RecipeCase::LookupI8 => "replacement-lookup",
                RecipeCase::LinearI8 => "replacement-linear-i8",
                RecipeCase::LinearCi8 => "replacement-linear-ci8",
            },
            &fixture,
            &harness,
            &pixels,
            &expected,
            &expected,
        );
    }
}

fn artwork() -> Vec<u8> {
    (0..6)
        .flat_map(|y| {
            (0..6).flat_map(move |x| {
                [
                    16 + x * 28,
                    24 + y * 32,
                    8 + x * 12 + y * 8,
                    48 + x * 16 + y * 12,
                ]
            })
        })
        .collect()
}

fn scaled_fixture(filter: u32, mirror: bool) -> Fixture {
    let mut fixture = recipe_fixture(RecipeCase::LinearI8, false);
    let mut list = commands(&fixture);
    let rect = list.iter().position(|(w, _)| w >> 24 == 0xe4).unwrap();
    list[rect].0 = 0xe400_0000 | ((16 + 6) * 4) << 12 | ((24 + 6) * 4);
    list[rect + 1].1 = if filter == 0 { 0 } else { 0x0004_0004 };
    list[rect + 2].1 = 0x0200_0200;
    list.insert(rect, gsp_setothermode_h_f3d(12, 2, filter << 12));
    if mirror {
        // A two-texel mirror period is contained in the three-texel logical image.
        for (w0, w1) in &mut list {
            if *w0 >> 24 == 0xf5 && *w1 >> 24 == 0 {
                *w1 = (*w1 & !((3 << 8) | (15 << 4) | (3 << 18) | (15 << 14)))
                    | (1 << 8)
                    | (1 << 4)
                    | (1 << 18)
                    | (1 << 14);
            }
        }
    }
    let pass = n64_gbi::encode::CcPass {
        a: n64_gbi::encode::ZERO_C,
        b: n64_gbi::encode::ZERO_C,
        c: n64_gbi::encode::ZERO_C,
        d: 1,
    };
    let alpha = n64_gbi::encode::CcPass {
        a: n64_gbi::encode::ZERO_A,
        b: n64_gbi::encode::ZERO_A,
        c: n64_gbi::encode::ZERO_A,
        d: 1,
    };
    for command in &mut list {
        if command.0 >> 24 == 0xfc {
            *command = n64_gbi::encode::gdp_set_combine_lerp(pass, alpha, pass, alpha);
        }
        if command.0 == 0xb900_031d {
            command.1 = 0;
        }
    }
    replace_commands(&mut fixture, list);
    fixture
}

fn scale_native() -> Vec<u8> {
    let mut pixels = expected_color([61; 3], 6, 6);
    for y in 24..30 {
        for x in 16..22 {
            pixels[(y * 320 + x) * 4 + 3] = 61;
        }
    }
    pixels
}

fn scaled_expected(filter: u32, mirror: bool) -> Vec<u8> {
    let art = artwork();
    let mut expected = [0, 0, 0, 255].repeat(320 * 240);
    let axis = |i: usize| {
        if mirror {
            [0, 1, 2, 3, 3, 2, 1][i]
        } else {
            i.min(5)
        }
    };
    for y in 0..6 {
        for x in 0..6 {
            let sample = |dx, dy, c| u32::from(art[(axis(y + dy) * 6 + axis(x + dx)) * 4 + c]);
            for c in 0..4 {
                let value = if filter == 0 {
                    sample(0, 0, c)
                } else {
                    (sample(0, 0, c) * 2 + sample(1, 0, c) + sample(0, 1, c)) / 4
                };
                expected[((24 + y) * 320 + 16 + x) * 4 + c] = value as u8;
            }
        }
    }
    expected
}

async fn scale_mapping() {
    for (filter, mirror, id) in [
        (0, false, "replacement-2x-point"),
        (2, false, "replacement-2x-three-point"),
        (0, true, "replacement-2x-mirror-point"),
        (2, true, "replacement-2x-mirror-three-point"),
    ] {
        let fixture = scaled_fixture(filter, mirror);
        let request = requests(&fixture).remove(0);
        let target = target(&request, TextureKeyScheme::Fast3dV1).unwrap();
        let mut harness = Harness::new(&fixture).await;
        harness.renderer.begin_frame();
        harness.process(&fixture, None);
        let before = harness.pixels().await;
        assert_eq!(before, scale_native());
        harness.recorder.drain();
        harness
            .renderer
            .set_replacement_provider(Some(Arc::new(ReplacementProvider::new(
                1,
                [ready(target, [6, 6], artwork())],
            ))));
        harness.renderer.begin_frame();
        harness.process(&fixture, None);
        let counts = harness.recorder.drain();
        assert_eq!(count(&counts, "replacement.upload_calls"), 1);
        assert_eq!(count(&counts, "replacement.upload_bytes"), 144);
        assert_eq!(count(&counts, "tmem.decode_dispatches"), 0);
        #[cfg(not(target_arch = "wasm32"))]
        export_snapshot(id, &counts);
        let pixels = harness.pixels().await;
        export_case(
            id,
            &fixture,
            &harness,
            &pixels,
            &scaled_expected(filter, mirror),
            &before,
        );
        harness.recorder.drain();
        harness.renderer.begin_frame();
        harness.process(&fixture, None);
        let warm = harness.recorder.drain();
        assert_eq!(count(&warm, "replacement.provider_calls"), 0);
        assert_eq!(count(&warm, "replacement.upload_calls"), 0);
        assert_eq!(count(&warm, "replacement.v5_hashes"), 0);
        #[cfg(not(target_arch = "wasm32"))]
        export_snapshot(&format!("{id}-warm"), &warm);
        assert_eq!(harness.pixels().await, pixels);
    }
}

gpu_test!(replacement_revision_pending_removal_and_epoch, lifecycle);
gpu_test!(
    replacement_identity_lookup_banks_roles_and_v5,
    identity_roles_and_aliases
);
gpu_test!(
    replacement_asymmetric_scale_point_three_point_mirror_clamp,
    scale_mapping
);

#[test]
fn replacement_fixture_targets_are_proved_before_rendering() {
    let layout = fixture("tmem-layouts");
    for request in requests(&layout) {
        assert!(
            target(&request, TextureKeyScheme::Rt64V5).is_ok(),
            "{:?}",
            request.tile
        );
    }
    for (filter, mirror) in [(0, false), (2, false), (0, true), (2, true)] {
        let fixture = scaled_fixture(filter, mirror);
        let request = requests(&fixture).remove(0);
        assert_eq!(request.extent, [3, 3]);
        assert_eq!(request.representation, Representation::Linear);
        assert!(target(&request, TextureKeyScheme::Fast3dV1).is_ok());
        assert!(target(&request, TextureKeyScheme::Rt64V5).is_err());
        let expected = scaled_expected(filter, mirror);
        let before = scale_native();
        let changes: Vec<_> = expected
            .as_chunks::<4>()
            .0
            .iter()
            .zip(before.as_chunks::<4>().0)
            .enumerate()
            .filter_map(|(i, (a, b))| (a != b).then_some(i))
            .collect();
        assert_eq!(changes.len(), 36);
        assert_eq!(changes[0], 24 * 320 + 16);
        assert_eq!(changes[35], 29 * 320 + 21);
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn replacement_literal_inventory() {
    let native = expected_recipe_pixels(RecipeCase::LinearI8, false);
    let blue = expected_color([8, 40, 232], 3, 3);
    let red = expected_color([224, 16, 32], 3, 3);
    let mut rows = Vec::new();
    for (id, expected) in [
        ("replacement-off", native.clone()),
        ("replacement-empty", native.clone()),
        ("replacement-pending", native.clone()),
        ("replacement-identity", native.clone()),
        ("replacement-blue", blue.clone()),
        ("replacement-red-same-revision", red),
        ("replacement-removed", native.clone()),
        ("replacement-invalid-extent", native.clone()),
        ("replacement-invalid-format", native.clone()),
        ("replacement-frame-frozen", blue),
        ("replacement-new-epoch", native.clone()),
    ] {
        rows.push((id, expected, native.clone()));
    }
    for (id, fixture) in [
        ("replacement-formats", "tmem-layouts"),
        ("replacement-v5-formats", "tmem-layouts"),
        ("replacement-roles", "tmem-roles-lifetime"),
        ("replacement-palette-banks", "tmem-tlut-mutation"),
    ] {
        let expected = semantics::expected_pixels(fixture);
        rows.push((id, expected.clone(), expected));
    }
    for (id, recipe) in [
        ("replacement-lookup", RecipeCase::LookupI8),
        ("replacement-linear-i8", RecipeCase::LinearI8),
        ("replacement-linear-ci8", RecipeCase::LinearCi8),
    ] {
        let expected = expected_recipe_pixels(recipe, false);
        rows.push((id, expected.clone(), expected));
    }
    for (filter, mirror, id) in [
        (0, false, "replacement-2x-point"),
        (2, false, "replacement-2x-three-point"),
        (0, true, "replacement-2x-mirror-point"),
        (2, true, "replacement-2x-mirror-three-point"),
    ] {
        rows.push((id, scaled_expected(filter, mirror), scale_native()));
    }
    for (id, draw, before, after) in [
        ("replacement-target-tex1", 0, 91, 239),
        ("replacement-target-lod1", 0, 37, 37),
        ("replacement-target-detail", 1, 37, 239),
    ] {
        rows.push((id, role_expected(draw, after), role_expected(draw, before)));
    }
    assert_eq!(rows.len(), 25);
    if let Some(directory) = std::env::var_os("FAST3D_REPLACEMENT_EXPECTATIONS") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        for (id, expected, before) in rows {
            let mask: Vec<_> = expected
                .as_chunks::<4>()
                .0
                .iter()
                .zip(before.as_chunks::<4>().0)
                .map(|(a, b)| u8::from(a != b))
                .collect();
            std::fs::write(directory.join(format!("{id}.expected.bin")), expected).unwrap();
            std::fs::write(directory.join(format!("{id}.before.bin")), before).unwrap();
            std::fs::write(directory.join(format!("{id}.mask.bin")), mask).unwrap();
        }
    }
}

fn role_expected(draw: usize, value: u8) -> Vec<u8> {
    let mut expected = [0, 0, 0, 255].repeat(320 * 240);
    for y in 112..128 {
        for x in 136 + draw * 16..152 + draw * 16 {
            expected[(y * 320 + x) * 4..(y * 320 + x + 1) * 4]
                .copy_from_slice(&[value, value, value, 255]);
        }
    }
    expected
}

async fn targeted_roles() {
    for (role, before_value, after_value, draw, id) in [
        ("texture1", 91, 239, 0, "replacement-target-tex1"),
        ("lod1", 37, 37, 0, "replacement-target-lod1"),
        ("detail", 37, 239, 1, "replacement-target-detail"),
    ] {
        let fixture = one_role(role);
        let request = requests(&fixture)
            .into_iter()
            .find(|request| request.role == role)
            .unwrap();
        let target = target(&request, TextureKeyScheme::Fast3dV1).unwrap();
        let extent = target.extent();
        let provider = Arc::new(ReplacementProvider::new(
            1,
            [ready(
                target,
                extent,
                vec![239; extent[0] as usize * extent[1] as usize * 4],
            )],
        ));
        let mut harness = Harness::new(&fixture).await;
        harness.renderer.begin_frame();
        harness.process(&fixture, None);
        let before = harness.pixels().await;
        assert_eq!(before, role_expected(draw, before_value));
        harness.recorder.drain();
        harness.renderer.set_replacement_provider(Some(provider));
        harness.renderer.begin_frame();
        harness.process(&fixture, None);
        let counts = harness.recorder.drain();
        #[cfg(not(target_arch = "wasm32"))]
        export_snapshot(id, &counts);
        assert_eq!(count(&counts, "replacement.matches"), 1, "{id}");
        assert_eq!(count(&counts, "replacement.upload_calls"), 1, "{id}");
        assert_eq!(count(&counts, "tmem.decode_dispatches"), 0, "{id}");
        let pixels = harness.pixels().await;
        export_case(
            id,
            &fixture,
            &harness,
            &pixels,
            &role_expected(draw, after_value),
            &before,
        );
    }
}

gpu_test!(
    replacement_changes_only_the_exact_independent_role,
    targeted_roles
);

#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn replacement_shared_fixture_v5_inputs() {
    let mut rows = Vec::new();
    for name in ["tmem-layouts", "tmem-roles-lifetime", "tmem-tlut-mutation"] {
        for (index, request) in requests(&fixture(name)).iter().enumerate() {
            let Ok(target) = target(request, TextureKeyScheme::Rt64V5) else {
                continue;
            };
            if request.tile.fmt != 2 && request.tlut != 0 {
                continue;
            }
            let hex = |bytes: &[u8]| {
                bytes
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            };
            rows.push(serde_json::json!({
                "name":format!("{name}-{}-{index}",request.role),
                "width":request.extent[0],"height":request.extent[1],"fmt":request.tile.fmt,"siz":request.tile.siz,
                "base":request.tile.tmem_addr,"line":request.tile.line,
                "palette":if request.tile.fmt==2 && request.tile.siz==0 {request.tile.palette} else {0},
                "tlut":request.tlut,"bank":hex(&request.bank.bytes),
                "xxh3_64":format!("{:016x}",target.key().xxh3_64),"stream":hex(target.witness()),
            }));
        }
    }
    let frozen: serde_json::Value =
        serde_json::from_str(include_str!("../../../tools/tmem/rt64-v5-fixtures.json")).unwrap();
    let expected = frozen["rows"].as_array().unwrap();
    assert_eq!(rows.len(), expected.len());
    for (row, expected) in rows.iter().zip(expected) {
        assert_eq!(row["name"], expected["name"]);
        assert_eq!(row["xxh3_64"], expected["xxh3_64"]);
        assert_eq!(row["stream"], expected["stream"]);
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(path) = std::env::var_os("FAST3D_V5_FIXTURE_INPUTS") {
        std::fs::write(path, serde_json::to_vec_pretty(&rows).unwrap()).unwrap();
    }
}
