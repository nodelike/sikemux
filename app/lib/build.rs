use std::path::Path;

/// Why the build stopped, printed as is so a shader's error keeps its lines.
struct Failed(String);

impl std::fmt::Debug for Failed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

type Result<T = ()> = std::result::Result<T, Failed>;

fn failed(what: impl std::fmt::Display) -> impl FnOnce(std::io::Error) -> Failed {
    move |error| Failed(format!("{what}: {error}"))
}

fn read(path: impl AsRef<Path>) -> Result<String> {
    let path = path.as_ref();
    std::fs::read_to_string(path).map_err(failed(path.display()))
}

fn out_dir() -> Result<std::path::PathBuf> {
    std::env::var("OUT_DIR")
        .map(Into::into)
        .map_err(|error| Failed(format!("OUT_DIR: {error}")))
}

fn main() -> Result {
    faces()?;
    shaders()?;
    strings()
}

/// The languages the app speaks besides English, as the catalog tags them, each named in itself.
/// A language comes back here once someone has reviewed its translation.
const LANGUAGES: [(&str, &str); 2] = [("es", "Español"), ("fr", "Français")];

/// Turns the string catalog into a table per language of each English string and what it
/// says there, sorted by the English for `i18n::tr`'s binary search.
fn strings() -> Result {
    let path = "../locales/Localizable.xcstrings";
    println!("cargo:rerun-if-changed={path}");
    let catalog: serde_json::Value = serde_json::from_str(&read(path)?)
        .map_err(|error| Failed(format!("the string catalog is not JSON: {error}")))?;
    let strings = catalog
        .get("strings")
        .and_then(|strings| strings.as_object())
        .ok_or_else(|| Failed("the catalog has no strings".to_owned()))?;
    let mut out = String::from("pub static LANGUAGES: &[Language] = &[\n");
    for (code, name) in LANGUAGES {
        let mut words: Vec<(&str, &str)> = strings
            .iter()
            .filter_map(|(english, entry)| {
                let said = entry
                    .get("localizations")?
                    .get(code)?
                    .get("stringUnit")?
                    .get("value")?
                    .as_str()?;
                Some((english.as_str(), said))
            })
            .collect();
        words.sort();
        out.push_str(&format!(
            "    Language {{ code: {code:?}, name: {name:?}, words: &[\n"
        ));
        for (english, said) in words {
            out.push_str(&format!("        ({english:?}, {said:?}),\n"));
        }
        out.push_str("    ] },\n");
    }
    out.push_str("];\n");
    std::fs::write(out_dir()?.join("strings.rs"), out).map_err(failed("write strings.rs"))
}

fn faces() -> Result {
    println!("cargo:rerun-if-changed=../assets/faces");
    let mut shapes = Vec::new();
    for entry in std::fs::read_dir("../assets/faces").map_err(failed("../assets/faces"))? {
        let name = entry
            .map_err(failed("a face file"))?
            .file_name()
            .into_string()
            .map_err(|name| Failed(format!("{name:?} is not a file name")))?;
        if name.ends_with(".json.gz") {
            shapes.push(name);
        }
    }
    shapes.sort();
    let entries: String = shapes
        .iter()
        .map(|file| {
            let shape = file.trim_end_matches(".json.gz");
            format!("    ({shape:?}, include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/../assets/faces/{file}\"))),\n")
        })
        .collect();
    std::fs::write(
        out_dir()?.join("faces.rs"),
        format!("static DRAWINGS: &[(&str, &[u8])] = &[\n{entries}];\n"),
    )
    .map_err(failed("write faces.rs"))
}

/// Puts the shared code and the backdrop kinds, which the phones draw too, in front of each
/// shader in `assets/shaders`, and translates the whole to Metal Shading Language for the
/// Mac's renderer, with the bindings GPUI hands an app shader at the buffers it sets.
fn shaders() -> Result {
    use naga::back::msl;

    println!("cargo:rerun-if-changed=../assets/shaders");
    let out = out_dir()?.join("shaders");
    std::fs::create_dir_all(&out).map_err(failed("create the shaders folder"))?;
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
    let mut kinds = Vec::new();
    for entry in
        std::fs::read_dir("../assets/shaders/kinds").map_err(failed("../assets/shaders/kinds"))?
    {
        let path = entry.map_err(failed("a kind's file"))?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "wgsl")
        {
            kinds.push(path);
        }
    }
    kinds.sort();
    let mut shared = read("../assets/shaders/dither.wgsl")?;
    for kind in &kinds {
        shared = shared + "\n" + &read(kind)?;
    }
    shared += "\n";
    for entry in std::fs::read_dir("../assets/shaders").map_err(failed("../assets/shaders"))? {
        let path = entry.map_err(failed("a shader file"))?.path();
        if path.extension().is_none_or(|extension| extension != "wgsl")
            || path.file_stem().is_some_and(|stem| stem == "dither")
        {
            continue;
        }
        let source = shared.clone() + &read(&path)?;
        let shown = path.display().to_string();
        let module = naga::front::wgsl::parse_str(&source)
            .map_err(|error| Failed(error.emit_to_string_with_path(&source, &shown)))?;
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .map_err(|error| Failed(error.emit_to_string_with_path(&source, &shown)))?;
        let (metal, translated) =
            msl::write_string(&module, &info, &options, &msl::PipelineOptions::default())
                .map_err(|error| Failed(format!("{shown}: {error}")))?;
        for name in translated.entry_point_names {
            let name = name.map_err(|error| Failed(format!("{shown}: {error}")))?;
            if name != "vs_main" && name != "fs_main" {
                return Err(Failed(format!(
                    "{shown}: entry point {name} is neither vs_main nor fs_main"
                )));
            }
        }
        let Some(stem) = path.file_stem() else {
            return Err(Failed(format!("{shown} has no name")));
        };
        let name = out.join(stem);
        std::fs::write(name.with_extension("wgsl"), &source).map_err(failed("write a shader"))?;
        std::fs::write(name.with_extension("metal"), metal).map_err(failed("write a shader"))?;
    }
    Ok(())
}
