fn main() {
    println!("cargo:rerun-if-env-changed=CORD_VERSION");

    // Release builds run before the version bump is committed and pass the
    // upcoming version explicitly.
    let version = std::env::var("CORD_VERSION")
        .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string());
    println!("cargo:rustc-env=CORD_VERSION={version}");
}
