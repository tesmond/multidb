use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let dist_dir = manifest_dir.join("../frontend/dist");
    let out_file = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("embedded_assets.rs");
    let windows_icon = manifest_dir.join("../build/windows/icon.ico");

    println!("cargo:rerun-if-changed={}", dist_dir.display());
    println!("cargo:rerun-if-changed={}", windows_icon.display());
    println!("cargo:rerun-if-changed=app.rc");

    #[cfg(windows)]
    embed_resource::compile("app.rc", embed_resource::NONE)
        .manifest_optional()
        .expect("embed Windows application icon");

    let mut assets = Vec::new();
    if dist_dir.join("index.html").is_file() {
        collect_assets(&dist_dir, &dist_dir, &mut assets);
    }
    assets.sort_by(|left, right| left.0.cmp(&right.0));

    let mut code = String::from("pub const EMBEDDED_ASSETS: &[(&str, &[u8])] = &[\n");
    for (relative, path) in assets {
        code.push_str(&format!(
            "    ({relative:?}, include_bytes!(r###\"{}\"###)),\n",
            path.display()
        ));
    }
    code.push_str("];\n");

    fs::write(out_file, code).expect("write embedded asset manifest");
}

fn collect_assets(root: &Path, dir: &Path, assets: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_assets(root, &path, assets);
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        assets.push((relative, path));
    }
}
