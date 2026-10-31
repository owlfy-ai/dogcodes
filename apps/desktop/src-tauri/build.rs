fn main() {
    println!(
        "cargo:rustc-env=DOGCODES_BUILD_TARGET={}",
        std::env::var("TARGET").expect("Cargo target")
    );
    // Tauri embeds cached icon bytes; track their sources so replacing an icon
    // also refreshes the macOS Dock icon on the next development build.
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build();
}
