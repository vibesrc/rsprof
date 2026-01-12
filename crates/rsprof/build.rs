fn main() {
    #[cfg(feature = "heap")]
    build_bpf();
}

#[cfg(feature = "heap")]
fn build_bpf() {
    use std::env;
    use std::path::PathBuf;
    use std::process::Command;

    use libbpf_cargo::SkeletonBuilder;

    const SRC: &str = "src/heap/bpf/heap.bpf.c";

    let out_dir = PathBuf::from(
        env::var_os("OUT_DIR").expect("OUT_DIR must be set in build script"),
    );

    let skel_path = out_dir.join("heap.skel.rs");

    // Check if clang is available
    let clang_available = Command::new("clang")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !clang_available {
        panic!(
            "clang not found - required for heap profiling.\n\
             Install with: sudo apt install clang libbpf-dev\n\
             Or disable heap profiling: cargo build --no-default-features"
        );
    }

    // Only build eBPF if the source file exists
    if !std::path::Path::new(SRC).exists() {
        panic!("eBPF source not found at {}", SRC);
    }

    SkeletonBuilder::new()
        .source(SRC)
        .clang_args([
            "-I",
            "src/heap/bpf",
            "-Wno-compare-distinct-pointer-types",
            "-g",
            "-O2",
        ])
        .build_and_generate(&skel_path)
        .unwrap_or_else(|e| {
            panic!(
                "Failed to build eBPF program: {}\n\
                 Make sure clang and libbpf-dev are installed:\n\
                 sudo apt install clang libbpf-dev",
                e
            );
        });

    println!("cargo:rerun-if-changed={}", SRC);
    println!("cargo:rerun-if-changed=src/heap/bpf/vmlinux.h");
}
