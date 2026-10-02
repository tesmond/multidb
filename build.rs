fn main() {
    println!("cargo:rerun-if-changed=icons/icon.ico");
    println!("cargo:rerun-if-changed=app.rc");

    #[cfg(windows)]
    embed_resource::compile("app.rc", embed_resource::NONE)
        .manifest_optional()
        .expect("embed Windows application icon");
}
