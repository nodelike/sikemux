use std::path::Path;

fn main() {
    faces();
    shaders();
}

fn faces() {
    println!("cargo:rerun-if-changed=assets/faces");
    let mut shapes: Vec<String> = std::fs::read_dir("assets/faces")
        .expect("assets/faces")
        .map(|entry| {
            entry
                .expect("a face file")
                .file_name()
                .into_string()
                .expect("a file name")
        })
        .filter(|name| name.ends_with(".json.gz"))
        .collect();
    shapes.sort();
    let entries: String = shapes
        .iter()
        .map(|file| {
            let shape = file.trim_end_matches(".json.gz");
            format!("    ({shape:?}, include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/assets/faces/{file}\"))),\n")
        })
        .collect();
    let out = Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("faces.rs");
    std::fs::write(
        out,
        format!("static DRAWINGS: &[(&str, &[u8])] = &[\n{entries}];\n"),
    )
    .expect("write faces.rs");
}

/// Puts the shared dithering code in front of each shader in `assets/shaders`, and
/// translates the whole to Metal Shading Language for the Mac's renderer, with the
/// bindings GPUI hands an app shader at the buffers it sets.
fn shaders() {
    use naga::back::msl;

    println!("cargo:rerun-if-changed=assets/shaders");
    let out = Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("shaders");
    std::fs::create_dir_all(&out).expect("create the shaders folder");
    let at = |group, binding, target| (naga::ResourceBinding { group, binding }, target);
    let resources = msl::EntryPointResources {
        resources: [
            at(
                0,
                0,
                msl::BindTarget {
                    buffer: Some(0),
                    ..Default::default()
                },
            ),
            at(
                1,
                0,
                msl::BindTarget {
                    buffer: Some(1),
                    ..Default::default()
                },
            ),
            at(
                2,
                0,
                msl::BindTarget {
                    texture: Some(0),
                    ..Default::default()
                },
            ),
            at(
                2,
                1,
                msl::BindTarget {
                    sampler: Some(msl::BindSamplerTarget::Inline(0)),
                    ..Default::default()
                },
            ),
        ]
        .into_iter()
        .collect(),
        sizes_buffer: Some(2),
        ..Default::default()
    };
    let linear = msl::sampler::InlineSampler {
        coord: msl::sampler::Coord::Normalized,
        address: [msl::sampler::Address::ClampToEdge; 3],
        border_color: msl::sampler::BorderColor::TransparentBlack,
        mag_filter: msl::sampler::Filter::Linear,
        min_filter: msl::sampler::Filter::Linear,
        mip_filter: None,
        lod_clamp: None,
        max_anisotropy: None,
        compare_func: msl::sampler::CompareFunc::Never,
    };
    let options = msl::Options {
        lang_version: (2, 0),
        per_entry_point_map: ["vs_main", "fs_main"]
            .map(|entry| (entry.to_owned(), resources.clone()))
            .into_iter()
            .collect(),
        inline_samplers: vec![linear],
        fake_missing_bindings: false,
        ..Default::default()
    };
    let shared = std::fs::read_to_string("assets/shaders/dither.wgsl").expect("dither.wgsl");
    for entry in std::fs::read_dir("assets/shaders").expect("assets/shaders") {
        let path = entry.expect("a shader file").path();
        if path.extension().is_none_or(|extension| extension != "wgsl")
            || path.file_stem().is_some_and(|stem| stem == "dither")
        {
            continue;
        }
        let source = shared.clone() + &std::fs::read_to_string(&path).expect("read a shader");
        let shown = path.display().to_string();
        let module = naga::front::wgsl::parse_str(&source)
            .unwrap_or_else(|error| panic!("{}", error.emit_to_string_with_path(&source, &shown)));
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap_or_else(|error| panic!("{}", error.emit_to_string_with_path(&source, &shown)));
        let (metal, translated) =
            msl::write_string(&module, &info, &options, &msl::PipelineOptions::default())
                .unwrap_or_else(|error| panic!("{shown}: {error}"));
        for name in translated.entry_point_names {
            let name = name.unwrap_or_else(|error| panic!("{shown}: {error}"));
            assert!(
                name == "vs_main" || name == "fs_main",
                "{shown}: entry point {name} is neither vs_main nor fs_main"
            );
        }
        let name = out.join(path.file_stem().expect("a shader name"));
        std::fs::write(name.with_extension("wgsl"), &source).expect("write a shader");
        std::fs::write(name.with_extension("metal"), metal).expect("write a shader");
    }
}
