//! hello-bin —— **外部程序**（RAMFS 上的 `/bin/hello`，v0.9 第三步）
//!
//! 它不是内核直接跑的（`/init`、`/bin/sh` 那种），而是**被 Shell 用
//! `fork` + `execve` 起来**的：这份代码证明了「进程模型可用」这件事，
//! 每一条输出都对应一个可断言的串口轨迹：
//!
//! | 输出 | 证明了什么 |
//! | ---- | ---- |
//! | `hello-bin: Hello from an external program!` | `execve("/bin/hello")` 真的把映像换掉了 |
//! | `hello-bin: pid=N` | 新进程有**自己的** PID（不是 Shell 的） |
//! | `hello-bin: argc=N` | `argc` 由内核经入口寄存器送达 `_start`（见 `libc::args`） |
//! | `hello-bin: argv[i]=...` | `argv` 字符串真的写进了这个进程的用户栈 |
//! | `hello-bin: envc=N` / `env PATH=/bin` | `envp` 同样经入口寄存器送达，且 `execve` 默认继承父环境 |
//! | 返回值 0 / 非 0 | 退出码由 `wait4` 交给父进程（Shell 打印 `exit code=`） |
//!
//! 与 `user/hello`（`/init`）的差异：不用堆、不碰文件系统 —— 外部程序应当尽量薄，
//! 让它验证的就是「进程」这件事本身，而不是别的子系统。
//!
//! **CKM-1 起它还是「普通主体」的样板**：CKM 组件表把 `/bin/hello` 登记为
//! `uid 2000`、**零能力位**（`docs/ckm.md` 3.1），所以它每一次对受保护前缀的写
//! 都会被内核拦下 —— 而同一份代码在 `/init`（uid 0 + `CAP_FS_SYSTEM_WRITE`）
//! 手里是成功的。两类失败在这里共存且**可区分**：
//!
//! | 操作 | 失败原因 | errno |
//! | ---- | ---- | ---- |
//! | 写 `/etc/**`（受保护前缀） | 能力不足（`CAP_FS_SYSTEM_WRITE`） | `-1` `EPERM` |
//! | `O_CREAT` 到 `/tmp`（目录 `0755`，他人无写位） | mode 不允许（v1.0 POSIX） | `-13` `EACCES` |
//! | 读 `/etc/motd`（`0644`） | —— | 成功（读不受能力约束） |
//!
//! 这三行合起来说明一件事：判定是真的在**按主体**做，而不是「一律拒绝」。
//!
//! **CSS-3 起还多三行（判定点 8）**：挂载 / 卸载 / 分配盘符都需要
//! `CAP_STORAGE_ADMIN`，而这个进程零能力 —— 于是三条都拿到 `-1`。
//! 与之成对的是 `/bin/volumes demo`（Shell 里 uid 1000，持有该能力）**成功**：
//! 同一组 syscall、同一个卷，唯一差别是主体。

#![no_std]
#![no_main]

use libc::{
    arg, argc, ckm_audit_read, ckm_policy_query, ckm_record_need, close, getpid, open, println,
    read, AUDIT_RECORD_SIZE, CAP_AUDIT_READ, CAP_FS_SYSTEM_WRITE, CKM_OP_AUDIT_READ,
    CKM_OP_FS_WRITE, CKM_OP_POLICY_QUERY, CKM_PREFIX_ETC, EACCES, EPERM, O_CREAT, O_RDONLY,
    O_WRONLY,
};

/// 内核通过 `_start` 调用该入口；返回值即退出码。
///
/// 退出码有意依赖参数个数：Shell 里敲 `hello` 得到 0，敲 `hello a b` 得到 2 ——
/// 于是「参数真的传到了子进程」在**退出码**上也能观察到，而不只是靠日志。
#[no_mangle]
pub extern "C" fn main() -> i32 {
    let pid = getpid();
    let n = argc();

    println!("hello-bin: Hello from an external program!");
    println!("hello-bin: pid={}", pid);
    println!("hello-bin: argc={}", n);
    for i in 0..n {
        match arg(i) {
            Some(bytes) => match core::str::from_utf8(bytes) {
                Ok(text) => println!("hello-bin: argv[{}]={}", i, text),
                Err(_) => println!("hello-bin: argv[{}]=<not utf-8>", i),
            },
            None => println!("hello-bin: argv[{}]=<missing>", i),
        }
    }
    env_report();
    ckm_denials();

    println!("hello-bin: done, exiting with {}", (n - 1) as i32);

    // 退出码 = 参数个数 - 1（`hello` → 0，`hello a b` → 2）
    (n - 1) as i32
}

/// v1.0：环境变量真的被 `execve` 搬进了这个新映像（`docs/syscall.md`）。
///
/// 顶层 Shell 的环境来自内核默认集合（`PATH=/bin`、`HOME=/`、`TERM=cerlesse`），
/// `/bin/hello` 又是被 Shell `fork` + `execve` 起来的 —— 因此这三行同时证明了
/// 「内核把 envp 送进了 `_start`」与「`execve` 默认让子进程继承环境」两件事。
fn env_report() {
    println!("hello-bin: envc={}", libc::env_count());
    for name in ["PATH", "HOME"] {
        match libc::getenv(name) {
            Some(value) => println!("hello-bin: env {}={}", name, value),
            None => println!("hello-bin: env {}=<unset>", name),
        }
    }
}

/// CKM-1：`uid 2000` 的**默认拒绘**在 ring3 上的证据（`docs/ckm.md` 3.4 判定点 1/5）。
///
/// 它不做断言、不改退出码（退出码属于参数传递这条验收线）；只把结果打出来，
/// 由 `make test-ckm` / `make test-proc` 在串口上逐行断言。
/// 每一条失败都是内核用**不同**的 errno 给的：能力不足 `-1`、mode 不允许 `-13`；
/// 这一对差异不能被合并（合并了就等于「分不清是没能力还是不属于你」）。
fn ckm_denials() {
    println!("hello-bin: ckm selftest begin (uid 2000, caps 0x0)");
    let mut rec = [0u8; AUDIT_RECORD_SIZE];

    // 1) 查自己：写受保护前缀需要 CAP_FS_SYSTEM_WRITE（0x4）—— 我没有
    let q = ckm_policy_query(CKM_OP_FS_WRITE, CKM_PREFIX_ETC, &mut rec);
    println!(
        "hello-bin: ckm policy_query(fs_write,etc) -> {} need=0x{:x}",
        q,
        ckm_record_need(&rec, 0)
    );

    // 2) 读审计需要 CAP_AUDIT_READ（0x8）—— 也没有
    let q = ckm_policy_query(CKM_OP_AUDIT_READ, 0, &mut rec);
    println!(
        "hello-bin: ckm policy_query(audit_read) -> {} need=0x{:x}",
        q,
        ckm_record_need(&rec, 0)
    );

    // 3) 查询接口自己无能力要求 → /init 与普通程序在这里都是允许的
    let q = ckm_policy_query(CKM_OP_POLICY_QUERY, 0, &mut rec);
    println!("hello-bin: ckm policy_query(policy_query) -> {}", q);

    // 4) 能力不足：写受保护前缀（即使路径不存在 → 也先被能力拦下，EPERM）
    let fd = open("/etc/hello-bin.flag", O_CREAT | O_WRONLY);
    println!("hello-bin: ckm open /etc/hello-bin.flag -> {} (EPERM expected)", fd);

    // 5) mode 不允许：`/tmp` 是 0755 root，他人没有写位（v1.0 的 EACCES）
    let fd = open("/tmp/hello-bin.flag", O_CREAT | O_WRONLY);
    println!("hello-bin: ckm open /tmp/hello-bin.flag -> {} (EACCES expected)", fd);

    // 6) 对照：同一个受保护目录里的**读**不受能力约束 → 成功
    let fd = open("/etc/motd", O_RDONLY);
    println!("hello-bin: ckm open /etc/motd ro -> {}", fd);
    if fd >= 0 {
        let mut buf = [0u8; 64];
        let n = read(fd as i32, &mut buf);
        println!("hello-bin: ckm read /etc/motd n={}", n);
        let _ = close(fd as i32);
    }

    // 7) 审计读取：需要 CAP_AUDIT_READ → 拒绘（返回 -1，不是「读到空」）
    let mut audit = [0u8; AUDIT_RECORD_SIZE];
    let n = ckm_audit_read(&mut audit, 0, None);
    println!("hello-bin: ckm audit_read -> {}", n);

    // 8) CSS-3：存储受控动作（判定点 8）—— 挂载/卸载/盘符都要 `CAP_STORAGE_ADMIN`（0x10）。
    //    三条都需要“三件套”拒绝（返回值 + 审计 + 串口证据），且都是**能力**拒绝（-1）
    //    而不是别的原因：卷标识取自卷表（真实存在），挂载点是个合法路径。
    let mut table = [0u8; 1024];
    let count = libc::storage_volumes(&mut table, None);
    let vol_id = if count > 0 {
        libc::volume_id(&table, 0)
    } else {
        "raw:0"
    };
    let mounted = libc::storage_mount(vol_id, "/volumes/hello-bin", 0);
    println!(
        "hello-bin: css storage_mount vol={} -> {} (EPERM expected)",
        vol_id, mounted
    );
    let unmounted = libc::storage_unmount("/volumes/hello-bin");
    println!(
        "hello-bin: css storage_unmount -> {} (EPERM expected)",
        unmounted
    );
    let letter = libc::storage_assign_letter(vol_id, b'Q');
    println!(
        "hello-bin: css storage_assign_letter Q -> {} (EPERM expected)",
        letter
    );

    // 把两个 errno 常量也打出来：若内核改了数值，这一行会立刻不同
    println!("hello-bin: ckm errno EPERM={} EACCES={}", EPERM, EACCES);
    println!(
        "hello-bin: ckm caps needed fs_write=0x{:x} audit_read=0x{:x}",
        CAP_FS_SYSTEM_WRITE, CAP_AUDIT_READ
    );
}
