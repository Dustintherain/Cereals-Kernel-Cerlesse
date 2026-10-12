//! 内核构建脚本。**纯内核**：本脚本不构建、不汇编任何用户态代码。
//!
//! 只做两件事：
//!
//! 1. 把 `link.ld` 交给内核二进制链接（绝对路径，避免 rustc 工作目录差异）；
//! 2. 把**用户态镜像**内嵌进内核映像 —— 输入是 `manifest.txt`（由 `make userland-images`
//!    在 `userland/` 里产出），输出是 `$OUT_DIR/userland_images.rs`：
//!
//! ```text
//! userland/Makefile ──▶ build/userland/images/{*.elf,ld.so,libgreet.so,dynhello,manifest.txt}
//!                                 │  manifest.txt: 每行 `P|L <RAMFS 路径> <文件名>`
//!                                 ▼
//! 本脚本：复制进 $OUT_DIR/userland/ + 生成 $OUT_DIR/userland_images.rs
//!                                 │  include!（syscall/user.rs）
//!                                 ▼
//! PROGRAM_IMAGES / LIBRARY_IMAGES ──▶ fs::init 写进 RAMFS（普通文件）──▶ ELF Loader
//! ```
//!
//! **没有用户态镜像时内核照样构建**：清单不存在（或 `CERLESSE_USERLAND_DIR` 显式置空）
//! 就生成空清单，启动后进入串口诊断模式（见 `syscall/user.rs::selftest`）——
//! 这是 docs/pure-kernel.md 验收标准 1/2/7。
//!
//! ## 为什么不再构建用户程序（v0.9 → 纯内核化）
//!
//! v0.9 及以前，本脚本用**嵌套 cargo** 构建 7 个用户程序、用 `as`/`ld` 现编动态链接
//! 夹具，并 `rerun-if-changed` 监视 `libc/`、`user/*`（27 条）。结果是「删掉用户态源码
//! 内核就编不出来」（实测 `cargo build -p kernel` 退出码 101），且内核构建需要 binutils。
//! 现在这些工作全部归 `userland/Makefile`；边界由 `tools/check-boundary.py`（`make check-boundary`）
//! 强制：本文件里不得出现 `Command::new`、不得出现用户态路径与包名。
//!
//! ## 构建期校验的分工（没有丢）
//!
//! 用户程序 ELF 的段布局校验（基址 / 页对齐 / W^X / 映像窗口 / 无可装载重定位）在
//! **用户态侧**：`userland/tools/check_user_elf.py`（由 `userland/Makefile` 对每个程序调用，
//! 动态链接夹具的结构校验在 `userland/tools/check_dyn_fixture.py`。本脚本只做
//! 「清单自洽 + 文件真的是 ELF」这一层轻校验；运行期装载器（`syscall/elf.rs`）仍会
//! 独立拒绝畸形映像（`user: ELF rejected: …`）。

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// 默认的用户态镜像目录（相对仓库根）：`make userland-images` 的产物。
const DEFAULT_USERLAND_DIR: &str = "build/userland/images";
/// 清单文件名（与 `userland/Makefile` 的 `MANIFEST` 成对维护）。
const MANIFEST: &str = "manifest.txt";
/// 生成源码的文件名（被 `syscall/user.rs` 用 `include!` 引入）。
const GENERATED: &str = "userland_images.rs";
/// 用户态镜像被复制到 `$OUT_DIR` 下的子目录。
const COPY_DIR: &str = "userland";

fn main() {
    let dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    println!("cargo:rustc-link-arg-bins=-T{dir}/link.ld");
    println!("cargo:rerun-if-changed=link.ld");
    // 切换「有/无用户态」是靠这个环境变量（`make image` vs `make image-kernel`），
    // 必须让 cargo 把它算进指纹，否则会复用上一次的产物。
    println!("cargo:rerun-if-env-changed=CERLESSE_USERLAND_DIR");

    let root = Path::new(&dir)
        .parent()
        .expect("kernel/ 应位于仓库根之下")
        .to_path_buf();
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));

    match resolve_userland_dir(&root) {
        Some(userland) => embed(&userland, &out_dir),
        None => {
            // 把**默认**清单路径也登记成输入。「先编内核、后出用户态镜像」确实会发生
            // （开发者的手顺 / 增量构建），而 cargo 只按已登记的输入判断要不要重跑本脚本：
            // 不登记这一条，清单从无到有时内核不会被重新构建，上面那句 warning 就一直是
            // 旧产物里的状态（表现为 `make test-user` 报 `user: no userland images embedded`）。
            // 登记一个当时还不存在的路径是允许的，它一旦被创建就会触发重跑。
            println!("cargo:rerun-if-changed={DEFAULT_USERLAND_DIR}/{MANIFEST}");
            write_generated(&out_dir, &[], &[]);
            println!(
                "cargo:warning=no userland images ({DEFAULT_USERLAND_DIR}/{MANIFEST} not found): \
                 building a kernel-only image; run `make userland-images` (or `make image`) to embed them"
            );
        }
    }
}

/// 解析用户态镜像目录。
///
/// - `CERLESSE_USERLAND_DIR` **已设置且为空** → 明确不内嵌（`make image-kernel` 走这条）；
/// - `CERLESSE_USERLAND_DIR` 指向某目录 → 该目录下必须有清单，否则报错（打错路径要立刻知道）；
/// - 未设置 → 回退到 `<仓库根>/build/userland/images`，没有清单就是不内嵌。
fn resolve_userland_dir(root: &Path) -> Option<PathBuf> {
    let manifest_exists = |dir: &Path| dir.join(MANIFEST).is_file();
    match env::var("CERLESSE_USERLAND_DIR") {
        Ok(value) if value.trim().is_empty() => None,
        Ok(value) => {
            let dir = PathBuf::from(value.trim());
            assert!(
                manifest_exists(&dir),
                "CERLESSE_USERLAND_DIR={} 下没有 {MANIFEST}：先在这个目录里跑 `make images`（见 userland/Makefile）",
                dir.display()
            );
            Some(dir)
        }
        Err(_) => {
            let dir = root.join(DEFAULT_USERLAND_DIR);
            if manifest_exists(&dir) {
                Some(dir)
            } else {
                None
            }
        }
    }
}

/// 清单里的一行：`<P|L> <RAMFS 路径> <文件名>`。
struct Entry {
    /// RAMFS 上的路径（`/init`、`/bin/sh`、`/lib/ld.so` …），由用户态侧决定。
    ramfs_path: String,
    /// 源文件（相对清单目录）。
    file: PathBuf,
}

/// 读取并校验清单：分成「可运行程序」与「被装载对象」两组。
fn read_manifest(dir: &Path) -> (Vec<Entry>, Vec<Entry>) {
    let text = fs::read_to_string(dir.join(MANIFEST))
        .unwrap_or_else(|err| panic!("读取 {} 失败：{err}", dir.join(MANIFEST).display()));
    let (mut programs, mut libraries) = (Vec::new(), Vec::new());
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let (kind, path, file) = match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some(k), Some(p), Some(f), None) => (k, p, f),
            _ => panic!("{}:{} 格式应为 `<P|L> <RAMFS 路径> <文件名>`：{line}", MANIFEST, n + 1),
        };
        assert!(
            path.starts_with('/'),
            "{MANIFEST}:{} RAMFS 路径必须是绝对路径：{path}",
            n + 1
        );
        let entry = Entry {
            ramfs_path: path.to_string(),
            file: dir.join(file),
        };
        assert!(
            entry.file.is_file(),
            "{MANIFEST}:{} 引用的文件不存在：{}（先跑 `make images`）",
            n + 1,
            entry.file.display()
        );
        match kind {
            "P" => programs.push(entry),
            "L" => libraries.push(entry),
            other => panic!("{MANIFEST}:{} 未知类型 {other:?}（只允许 P / L）", n + 1),
        }
    }
    assert!(
        programs.iter().all(|e| !e.ramfs_path.is_empty()),
        "{MANIFEST}: 程序路径不能为空"
    );
    (programs, libraries)
}

/// 复制镜像到 `$OUT_DIR/userland/` 并生成清单源码。
fn embed(userland: &Path, out_dir: &Path) {
    let (programs, libraries) = read_manifest(userland);
    let copy_dir = out_dir.join(COPY_DIR);
    fs::create_dir_all(&copy_dir).expect("无法创建 $OUT_DIR/userland");

    let mut copied = Vec::new();
    for entry in programs.iter().chain(libraries.iter()) {
        let name = entry
            .file
            .file_name()
            .expect("清单里的文件名")
            .to_string_lossy()
            .to_string();
        let dst = copy_dir.join(&name);
        let bytes = fs::read(&entry.file)
            .unwrap_or_else(|err| panic!("读取 {} 失败：{err}", entry.file.display()));
        // 轻校验：真的是 ELF、非空。段布局由用户态侧的 userland/tools/check_user_elf.py 负责。
        assert!(
            bytes.len() > 64 && &bytes[..4] == b"\x7fELF",
            "{} 不是 ELF 文件（清单第 `{}` 行）：谁产出的？检查 userland/Makefile",
            entry.file.display(),
            entry.ramfs_path
        );
        fs::write(&dst, &bytes)
            .unwrap_or_else(|err| panic!("写入 {} 失败：{err}", dst.display()));
        copied.push((entry.ramfs_path.clone(), dst));
    }

    let (programs, libraries) = copied.split_at(programs.len());
    write_generated(out_dir, programs, libraries);

    // 清单变化时重跑本脚本（`cargo:` 指令只对本目录内的路径做监视时最可靠，
    // 这里的路径是绝对路径 —— cargo 支持，用于跨目录依赖跟踪）。
    println!("cargo:rerun-if-changed={}", userland.join(MANIFEST).display());
    println!(
        "cargo:warning=embedded userland: {} programs + {} libraries from {}",
        programs.len(),
        libraries.len(),
        userland.display()
    );
}

/// 生成 `$OUT_DIR/userland_images.rs`（空清单也生成，保证 `include!` 永远能编过）。
fn write_generated(out_dir: &Path, programs: &[(String, PathBuf)], libraries: &[(String, PathBuf)]) {
    let mut src = String::from(
        "// 由 kernel/build.rs 生成 —— 不要手改。\n\
         //\n\
         // 用户态镜像清单（见 docs/pure-kernel.md 阶段四）：内容由 userland/ 的\n\
         // manifest.txt 决定；空清单 = 本次构建没有内嵌用户态（内核进入串口诊断模式）。\n\
         // 内核**不认识任何具体程序名**：路径来自清单，这里只是把它变成两个静态切片。\n",
    );
    for (name, entries) in [
        ("PROGRAM_IMAGES", programs),
        ("LIBRARY_IMAGES", libraries),
    ] {
        src.push_str(&format!(
            "\npub static {name}: &[(&str, &[u8])] = &[\n"
        ));
        for (path, file) in entries {
            // Windows 路径分隔符会把 include_bytes! 的字面量拆坏，统一成正斜杠。
            let literal = file.to_string_lossy().replace('\\', "/");
            src.push_str(&format!("    ({path:?}, include_bytes!({literal:?})),\n"));
        }
        src.push_str("];\n");
    }
    let dst = out_dir.join(GENERATED);
    fs::write(&dst, src).unwrap_or_else(|err| panic!("写入 {} 失败：{err}", dst.display()));
}
