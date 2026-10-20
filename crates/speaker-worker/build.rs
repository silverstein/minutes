fn main() {
    let target = std::env::var("CARGO_CFG_TARGET_OS").expect("Cargo target OS is required");
    if target == "linux" {
        // The opt-in Linux helper requires the system OpenBLAS development
        // package at build time and its shared library at run time.
        println!("cargo:rustc-link-lib=openblas");
    } else if target == "macos" {
        // ndarray's fallback also needs the legacy CBLAS interface on older
        // macOS versions where Speakrs' newer Accelerate thread API is absent.
        println!("cargo:rustc-link-lib=framework=Accelerate");
    } else if target == "windows" {
        windows_blas();
    }
}

#[cfg(windows)]
fn windows_blas() {
    assert!(
        std::env::var_os("VCPKGRS_DYNAMIC").is_none(),
        "the bound Windows helper requires statically linked OpenBLAS"
    );
    vcpkg::Config::new()
        .find_package("openblas")
        .expect("build the pinned OpenBLAS manifest with x64-windows-static-md first");
}

#[cfg(not(windows))]
fn windows_blas() {
    panic!("build the Windows helper natively with its pinned static OpenBLAS manifest");
}
