fn main() {
    // 让内核二进制使用本 crate 的链接脚本；绝对路径避免 rustc 工作目录差异。
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-arg-bins=-T{dir}/link.ld");
    println!("cargo:rerun-if-changed=link.ld");
}
