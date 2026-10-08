//! Stamp opt-in source installations without changing published release versions.
fn main() {
    println!("cargo:rerun-if-env-changed=YUPANA_LOCAL_BUILD");
    let mut version = std::env::var("CARGO_PKG_VERSION").expect("Cargo package version");
    if let Ok(identity) = std::env::var("YUPANA_LOCAL_BUILD") {
        let sha = identity.strip_suffix(".dirty").unwrap_or(&identity);
        assert!(
            sha.len() == 12 && sha.bytes().all(|b| b.is_ascii_hexdigit()),
            "YUPANA_LOCAL_BUILD must be a 12-digit Git hash, optionally followed by .dirty"
        );
        version.push_str("+local.");
        version.push_str(&identity);
    }
    println!("cargo:rustc-env=YUPANA_BUILD_VERSION={version}");
}
