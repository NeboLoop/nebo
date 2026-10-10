// macOS: embed an Info.plist (`assets/macos/nebo-cli-Info.plist`) in the
// binary's __TEXT,__info_plist section. The bare `nebo-darwin-<arch>` release
// asset is this binary, signed on its own (`dev.neboai.nebo.cli`): the plist
// is its identity outside any app bundle.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let plist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/macos/nebo-cli-Info.plist");
        println!("cargo:rerun-if-changed={}", plist.display());
        println!("cargo:rustc-link-arg-bin=nebo-cli=-Wl,-sectcreate,__TEXT,__info_plist,{}", plist.display());
    }
}
