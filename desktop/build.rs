use std::{env, path::PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let windows_icon = manifest_dir.join("../build/windows/icon.ico");
    let ui_file = manifest_dir.join("ui/main.slint");

    println!("cargo:rerun-if-changed={}", windows_icon.display());
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed={}", ui_file.display());

    #[cfg(windows)]
    embed_resource::compile("app.rc", embed_resource::NONE)
        .manifest_optional()
        .expect("embed Windows application icon");

    slint_build::compile(ui_file).expect("compile Slint UI");
}
