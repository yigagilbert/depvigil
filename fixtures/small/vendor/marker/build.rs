// If this build script ever runs, it leaves evidence behind. The Phase 0
// tests assert that the marker file does not exist.
use std::{env, fs, path::Path};

fn main() {
    let dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    fs::write(Path::new(&dir).join("BUILD_SCRIPT_RAN.marker"), "build script ran\n")
        .expect("write marker");
}
