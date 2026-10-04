fn main() {
    // Icon, version info and an asInvoker manifest for the Windows exe. Elevation is
    // requested per operation at runtime (see core::elevation), never for the whole app.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    println!("cargo:rerun-if-changed=windows/app.rc");
    println!("cargo:rerun-if-changed=windows/app.manifest");
    println!("cargo:rerun-if-changed=icon.ico");
    let result =
        embed_resource::compile("windows/app.rc", embed_resource::NONE).manifest_optional();
    if let Err(e) = result {
        // Only a cross-check from another OS may lack a resource compiler.
        if cfg!(windows) {
            panic!("embedding Windows resources failed: {e}");
        }
        println!("cargo:warning=Windows resources not embedded ({e})");
    }
}
