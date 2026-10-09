use std::path::PathBuf;

fn main() {
    // Icon, version info and an asInvoker manifest for the Windows exe. Elevation is
    // requested per operation at runtime (see core::elevation), never for the whole app.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    println!("cargo:rerun-if-changed=windows/app.rc");
    println!("cargo:rerun-if-changed=windows/app.manifest");
    println!("cargo:rerun-if-changed=icon.ico");
    let result = embed_resource::compile(versioned_rc(), embed_resource::NONE).manifest_optional();
    if let Err(e) = result {
        // Only a cross-check from another OS may lack a resource compiler.
        if cfg!(windows) {
            panic!("embedding Windows resources failed: {e}");
        }
        println!("cargo:warning=Windows resources not embedded ({e})");
    }
}

/// windows/app.rc with this version (Cargo.toml's) in its version info, written to OUT_DIR:
/// "0.11.10-beta.5" as the text, 0,11,10,0 as the numbers. Its files by absolute path, as
/// it's compiled from there.
fn versioned_rc() -> PathBuf {
    let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k} is not set"));
    let root = PathBuf::from(var("CARGO_MANIFEST_DIR"));
    let version = var("CARGO_PKG_VERSION");
    let mut numbers: Vec<&str> = version
        .split(['-', '+'])
        .next()
        .unwrap_or("0")
        .split('.')
        .collect();
    numbers.resize(4, "0");
    let rc = std::fs::read_to_string(root.join("windows/app.rc"))
        .expect("windows/app.rc")
        .replace("@ROOT@", &root.to_string_lossy().replace('\\', "/"))
        .replace("@NUMBERS@", &numbers.join(","))
        .replace("@VERSION@", &version);
    let out = PathBuf::from(var("OUT_DIR")).join("app.rc");
    std::fs::write(&out, rc).expect("writing app.rc");
    out
}
