//! Builds the pinned llama.cpp and generates bindings from its own headers.
//!
//! The pin is a release tag plus the SHA-256 of its source archive, so the
//! bytes compiled here are exactly the ones reviewed. llama.cpp gained the
//! EmbeddingGemma 2 architecture (`gemma-embedding2`) after the newest
//! llama-cpp-sys-2 release vendored it, which is why this crate builds its
//! own. `MINUTES_LLAMA_CPP_SRC` points at an already-extracted source tree for
//! offline builds; it must be the same tag.
#[cfg(target_os = "macos")]
use std::env;
#[cfg(target_os = "macos")]
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
const LLAMA_TAG: &str = "b11889";
#[cfg(target_os = "macos")]
const LLAMA_SHA256: &str = "634e76016ffb363659d160acb8ac70852e20b78ef11df43cd3221322c3a68f3b";

#[cfg(not(target_os = "macos"))]
fn main() {}

// The build dependencies are macOS-only, so so is this build script's body.
#[cfg(target_os = "macos")]
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=MINUTES_LLAMA_CPP_SRC");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let source = match env::var_os("MINUTES_LLAMA_CPP_SRC") {
        Some(path) => PathBuf::from(path),
        None => fetch_source(&out_dir),
    };

    let install = cmake::Config::new(&source)
        .profile("Release")
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("GGML_METAL", "ON")
        .define("GGML_METAL_EMBED_LIBRARY", "ON")
        .define("GGML_BLAS", "OFF")
        .define("GGML_OPENMP", "OFF")
        .define("GGML_NATIVE", "OFF")
        .define("LLAMA_BUILD_COMMON", "OFF")
        .define("LLAMA_BUILD_TESTS", "OFF")
        .define("LLAMA_BUILD_TOOLS", "OFF")
        .define("LLAMA_BUILD_EXAMPLES", "OFF")
        .define("LLAMA_BUILD_SERVER", "OFF")
        .define("LLAMA_BUILD_APP", "OFF")
        .define("LLAMA_BUILD_HTML", "OFF")
        .define("LLAMA_OPENSSL", "OFF")
        .define("CMAKE_OSX_DEPLOYMENT_TARGET", "11.0")
        .build();

    let lib_dir = install.join("lib");
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    for lib in ["llama", "ggml", "ggml-base", "ggml-cpu", "ggml-metal"] {
        println!("cargo:rustc-link-lib=static={lib}");
    }
    for framework in ["Foundation", "Metal", "MetalKit", "Accelerate"] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
    println!("cargo:rustc-link-lib=c++");
    link_clang_runtime();

    let include = install.join("include");
    bindgen::Builder::default()
        .header(include.join("llama.h").to_string_lossy())
        .clang_arg(format!("-I{}", include.display()))
        .allowlist_function("llama_.*")
        .allowlist_type("llama_.*")
        .layout_tests(false)
        .generate()
        .expect("generate llama.cpp bindings")
        .write_to_file(out_dir.join("llama_bindings.rs"))
        .expect("write llama.cpp bindings");
}

/// Downloads the pinned source archive, verifies its digest, and extracts it.
#[cfg(target_os = "macos")]
fn fetch_source(out_dir: &Path) -> PathBuf {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let source = out_dir.join(format!("llama.cpp-{LLAMA_TAG}"));
    if source.join("CMakeLists.txt").exists() {
        return source;
    }
    let url = format!("https://github.com/ggml-org/llama.cpp/archive/refs/tags/{LLAMA_TAG}.tar.gz");
    let mut archive = Vec::new();
    ureq::get(&url)
        .call()
        .unwrap_or_else(|e| panic!("download {url}: {e}"))
        .into_reader()
        .read_to_end(&mut archive)
        .expect("read llama.cpp archive");
    let digest = format!("{:x}", Sha256::digest(&archive));
    assert_eq!(
        digest, LLAMA_SHA256,
        "llama.cpp {LLAMA_TAG} archive digest mismatch; refusing to build unreviewed source"
    );
    tar::Archive::new(flate2::read::GzDecoder::new(archive.as_slice()))
        .unpack(out_dir)
        .expect("extract llama.cpp archive");
    source
}

/// ggml's Metal backend uses `@available`, which compiles to a call into
/// clang's runtime (`__isPlatformVersionAtLeast`). rustc links with
/// `-nodefaultlibs`, so that runtime has to be named explicitly.
#[cfg(target_os = "macos")]
fn link_clang_runtime() {
    let output = std::process::Command::new("clang")
        .arg("--print-resource-dir")
        .output()
        .expect("clang --print-resource-dir");
    let resource_dir = String::from_utf8(output.stdout).expect("utf-8 resource dir");
    println!(
        "cargo:rustc-link-search=native={}/lib/darwin",
        resource_dir.trim()
    );
    println!("cargo:rustc-link-lib=static=clang_rt.osx");
}
