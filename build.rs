use std::process::Command;
fn main() {
    // SpeexDSP echo canceller + preprocessor (BSD licence), built from source.
    let d = "third_party/speexdsp/libspeexdsp";
    cc::Build::new()
        .files(["mdf.c", "preprocess.c", "filterbank.c", "fftwrap.c", "smallft.c"].iter().map(|f| format!("{d}/{f}")))
        .include("third_party/speexdsp/include")
        .include(d)
        .define("FLOATING_POINT", None)
        .define("USE_SMALLFT", None)
        .define("EXPORT", Some(""))
        .opt_level(3)
        .warnings(false)
        .compile("speexdsp");
    println!("cargo:rerun-if-changed=third_party/speexdsp");

    // Windows icon
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=assets/icon.rc");
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("windows") { return; }
    let out = std::env::var("OUT_DIR").unwrap();
    let res = format!("{out}/icon.res.o");
    let ok = Command::new("x86_64-w64-mingw32-windres")
        .args(["--input", "assets/icon.rc", "--output-format=coff", "--output", &res, "--include-dir", "assets"])
        .status().map(|s| s.success()).unwrap_or(false);
    if ok { println!("cargo:rustc-link-arg-bins={res}"); }
}
