use std::{env, path::Path, process::Command};

fn main() {
    glib_build_tools::compile_resources(
        &["resources"],
        "resources/resources.gresource.xml",
        "spot_lyric.gresource",
    );

    compile_gsettings_schema("data/cn.spotlyric.Gtk.gschema.xml");
}

fn compile_gsettings_schema(schema: &str) {
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR must be set by Cargo");
    let schema_dir = Path::new(schema)
        .parent()
        .expect("GSettings schema must live in a directory");

    let output = Command::new("glib-compile-schemas")
        .arg("--strict")
        .arg("--targetdir")
        .arg(&out_dir)
        .arg(schema_dir)
        .output()
        .expect("failed to run glib-compile-schemas");

    assert!(
        output.status.success(),
        "glib-compile-schemas failed with exit status {} and stderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    println!("cargo:rerun-if-changed={schema}");
    println!("cargo:rustc-env=SPOT_LYRIC_GSETTINGS_SCHEMA_DIR={out_dir}");
}
