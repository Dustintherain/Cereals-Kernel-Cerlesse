//! hello —— v0.7 起的参考用户程序（也是 `/init`）
//!
//! 它不是「演示」而是**验收基准**：这段 Rust 编译出的 ELF 会由 `kernel/build.rs`
//! 内嵌，启动时写进 RAMFS 的 `/init`，再由内核的 ELF Loader 装载到 ring3 运行。
//! `make test-user` / `make test-fs` 的硬断言就落在这段程序的串口轨迹上：
//!
//! | 步骤 | 验证的能力 | 可断言的日志 |
//! | ---- | ---- | ---- |
//! | `println!` | `write` → 串口 | `user: write fd=1 len=22` |
//! | `getpid` | PID 传递 | `user: getpid -> 1` |
//! | `open/read/close /etc/motd` | VFS 读路径 | `user: open path=/etc/motd flags=0x0 fd=3`、`user: read fd=3 want=64 got=23`、首次读完后第二次读回 0（EOF） |
//! | `open(O_CREAT)/write /tmp/hello.txt` | VFS 写路径 | `user: write fd=3 len=24` + 内核读回 `fs: readback /tmp/hello.txt len=24` |
//! | `static mut` 自增 | `RW` 段对 ring3 可写 | `hello: static counter=1` |
//! | 非法指针 `write` | syscall 参数校验 | `user: EFAULT write ...` / `hello: bad write ret=-14` |
//! | `brk` / `mmap` / `munmap` | **用户堆**（v0.9 第一步） | `user: brk base=0x8000020000`、`user: mmap len=8192 ... -> 0x8000060000`、`hello: heap PASS` |
//! | `mmap` 第 6 参数 `offset` | syscall ABI 扩到 `r9`（v1.0）；匿名映射非 0 `offset` 明确拒绝 | `hello: mmap offset ret=-22`、`hello: mmap zero-offset ret=...` |
//! | **文件映射**（`offset` 真正生效） | v1.0 第三步：`fd` 指向的文件 `[offset,offset+len)` 映射、`offset` 页对齐 | `hello: filemmap contents=1`、`hello: filemmap misaligned ret=-22`、`hello: filemmap PASS=1` |
//! | **可写/共享文件映射 + `msync`** | v1.0 第五步：`MAP_PRIVATE` 改写不落盘；`MAP_SHARED` 在 `msync`/`munmap` 时写回文件（长度夹在文件大小内） | `user: msync addr=… -> pages=1 bytes=2048`、`hello: sharedmmap persisted=1`、`hello: sharedmmap PASS=1` |
//! | `Vec` / `format!` | `#[global_allocator]` 真的落到内核 | `hello: vec len=200 sum=20100` |
//! | `mprotect` | **运行时改保护属性**（v1.0 第一步）：`PROT_NONE` 关页 → 内核拒收该缓冲 → 恢复后内容原样 → 换 `RX` 并真的执行 | `user: mprotect addr=0x8000060000 len=4096 prot=0x0 -> pages=1`、`hello: mprotect none write ret=-14`、`hello: mprotect exec ret=42`、`hello: mprotect PASS=1` |
//! | `nanosleep` | tick 真实推进 | `user: sleep done ticks=2` |
//! | `exit(42)` | 退出码回收 | `user: exit code=42` |
//!
//! 编写新的用户程序时把它当作模板复制即可（`Cargo.toml` + `build.rs` +
//! `user.ld` + `src/main.rs`），约定见 `libc/src/lib.rs` 顶部文档。

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;

use libc::syscall::{raw, SYS_WRITE};
use libc::{
    bind, brk, ckm_audit_read, ckm_component_state, ckm_policy_query, ckm_record_is_deny,
    ckm_record_need, close, component_id, component_kind, component_kind_name, component_state,
    component_state_name, getpid, mmap, mmap_file, mmap_with_offset, mprotect, msync, munmap,
    nanosleep, net_config, open, println, read, recvfrom, sendto, socket, storage_rescan, write,
    AF_INET, AUDIT_RECORD_SIZE, CAP_CKM_SERVICE, CKM_OP_FS_WRITE, CKM_OP_POLICY_QUERY,
    CKM_OP_STATE_WRITE, CKM_PREFIX_ETC, COMPONENT_RECORD_SIZE, ENODEV, EPERM, EADDRINUSE,
    EAFNOSUPPORT, EINVAL, ENOTSOCK, EPROTONOSUPPORT, INADDR_BROADCAST, IPPROTO_UDP, MAP_ANONYMOUS,
    MAP_PRIVATE, MAP_SHARED, MS_INVALIDATE, MS_SYNC, O_CREAT, O_RDONLY, O_RDWR, O_WRONLY,
    PROT_EXEC, PROT_NONE, PROT_READ, PROT_WRITE, SOCK_DGRAM, SOCK_STREAM, STORAGE_RESCAN_ALL,
};
use libc::{AF_UNSPEC, NetConfig, SockAddrIn};

/// 可写的静态变量：v0.7 的 ELF Loader 把 `RW` 段映射成 ring3 可写，
/// 因此这里在运行期被改写（v0.6 会 #PF —— 代码页整体只读）。
static mut COUNTER: u32 = 0;

/// 内核提供的文件（`fs::init` 写入，23 字节）。
const MOTD_PATH: &str = "/etc/motd";
/// 用户程序写文件的落点（内核随后读回并打印内容）。
const OUT_PATH: &str = "/tmp/hello.txt";
/// CKM-1：`/init` 以 `CAP_FS_SYSTEM_WRITE` 在**受保护前缀**里建的文件。
///
/// 它与 `/bin/hello`（uid 2000）对同一个前缀的写入构成一对对照：
/// 同一个路径、同一个 flags，一个成功、一个 `EPERM` ——
/// 差别只在凭证，不在路径（`docs/ckm.md` 3.4 判定点 1）。
const INIT_FLAG_PATH: &str = "/etc/init.flag";

/// `brk` 一次增长两页（8 KiB）。
const HEAP_STEP: u64 = 2 * 4096;
/// 匿名映射长度（两页 = 8 KiB）。
const MMAP_LEN: usize = 2 * 4096;
/// 内核 mmap 窗口上限（256 KiB，见 `address_space::USER_MMAP_MAX`）。
const MMAP_MAX: usize = 64 * 4096;
/// 文件映射自测用的文件与长度（1 页 + 1 KiB，因此 `offset = 4096` 有意义）。
const MMAP_FILE_PATH: &str = "/tmp/mmap.bin";
const MMAP_FILE_LEN: usize = 5120;
/// 共享映射（`MAP_SHARED`）自测用的文件与长度：**2 KiB**，而映射是 4 KiB ——
/// 「映射比文件长」因此是可断言的（写回必须夹在文件大小内，不把文件撑大）。
const SHARED_FILE_PATH: &str = "/tmp/shared.bin";
const SHARED_FILE_LEN: usize = 2048;
/// `mprotect` 自测里要从 ring3 **执行**的机器码：`mov eax, 42; ret`。
///
/// 先以 `mmap(PROT_READ|PROT_WRITE)` 写进页里，再 `mprotect(RX)` 改成只读可执行 ——
/// 这就是 W^X 要求的「先写、再换只执行」，也是这份自测存在的理由。
static WX_CODE: [u8; 6] = [0xb8, 0x2a, 0x00, 0x00, 0x00, 0xc3];

/// 内核通过 `_start` 调用该入口；返回值即退出码。
#[no_mangle]
pub extern "C" fn main() -> i32 {
    // 22 字节，一次 write —— `make test-user` 断言 `user: write fd=1 len=22`。
    println!("hello from user space");

    // 展示 `{}` 格式化（PID 由 syscall 返回，再经 libc 格式化后写出）。
    let pid = getpid();
    println!("hello: pid={}", pid);

    // ---- 1. 从 RAMFS 读文件：open → read → write → read(EOF) → close ----
    let fd = open(MOTD_PATH, O_RDONLY);
    println!("hello: open {} fd={}", MOTD_PATH, fd);
    if fd >= 0 {
        let mut buf = [0u8; 64];
        let n = read(fd as i32, &mut buf);
        println!("hello: read n={}", n);
        if n > 0 {
            // 把读到的内容原样写到串口（内核日志里能看到 23 字节的那一次 write）
            let _ = write(1, &buf[..n as usize]);
        }
        // 第二次读：offset 已经在文件尾 → 0（EOF），证明偏移量真的在推进
        let eof = read(fd as i32, &mut buf);
        println!("hello: eof read n={}", eof);
        let _ = close(fd as i32);
    }

    // ---- 2. 往 RAMFS 写文件：内核在用户态结束后读回并打印 ----
    let wfd = open(OUT_PATH, O_CREAT | O_WRONLY);
    println!("hello: create {} fd={}", OUT_PATH, wfd);
    if wfd >= 0 {
        let msg = b"written by user program\n";
        let written = write(wfd as i32, msg);
        println!("hello: wrote {} bytes", written);
        let _ = close(wfd as i32);
    }

    // ---- 3. 可写静态数据（RW 段对 ring3 可写） ----
    let counter = unsafe {
        COUNTER += 1;
        COUNTER
    };
    println!("hello: static counter={}", counter);

    // ---- 4. 非法用户指针：内核校验用户缓冲，应返回 -EFAULT（-14） ----
    let bad = unsafe { raw(SYS_WRITE, 1, 0xdead_0000_0000, 1, 0, 0) };
    println!("hello: bad write ret={}", bad);

    // ---- 5. 用户堆：brk / mmap / munmap（v0.9 运行时前置条件） ----
    let heap_ok = heap_selftest();

    // ---- 5b. 文件映射：把文件的一段映射进内存（v1.0 第三步，`offset` 真正生效） ----
    let filemap_ok = file_mmap_selftest();

    // ---- 5c. 共享文件映射：MAP_SHARED 的改动写回文件（v1.0 第五步 + `msync`） ----
    let shared_ok = shared_mmap_selftest();

    // ---- 6. mprotect：PROT_NONE 关页 + RX 转换（v1.0 第一步） ----
    let prot_ok = mprotect_selftest();

    // ---- 7. nanosleep：2 个 tick = 20ms，内核在等待期间 `sti; hlt` ----
    let slept = nanosleep(2);
    println!("hello: slept={}", slept);

    if !heap_ok {
        println!("hello: heap FAIL");
        return 43;
    }
    if !filemap_ok {
        println!("hello: filemmap FAIL");
        return 46;
    }
    if !shared_ok {
        println!("hello: sharedmmap FAIL");
        return 48;
    }
    if !prot_ok {
        println!("hello: mprotect FAIL");
        return 44;
    }

    // ---- 8. CKM-1：凭证与能力判定（`/init` = uid 0 + CAP_FS_SYSTEM_WRITE） ----
    if !ckm_selftest() {
        println!("hello: ckm FAIL");
        return 45;
    }

    // ---- 9. v1.0：UDP 套接字最小集（socket/bind/sendto/recvfrom） ----
    // 只有在总线上有网卡、且内核 DHCP 成功时才跑；否则跳过（`make test-user` 无网卡）。
    if !socket_selftest() {
        println!("hello: socket FAIL");
        return 47;
    }
    42
}

/// UDP 套接字自测（v1.0）：走一次**真实的**、自包含的网络往返 —— 用 `socket`/
/// `bind`/`sendto`/`recvfrom` 向 QEMU slirp 的内置 DHCP 服务器发一条 DISCOVER，
/// 收到 OFFER 并校验它（不必主机侧跑任何服务，与内核网络自测同一条路径）。
///
/// 每类结果都留在串口上可断言：`hello: socket fd=`、`hello: bind ret=`、
/// `hello: net ip=… mac=…`、`hello: sendto ret=300`、`hello: recvfrom ret=… src=10.0.2.2:67`、
/// `hello: socket dhcp=1`、`hello: socket PASS=1`。
///
/// 没有网卡时 `net_config` 返回 [`ENODEV`]，直接跳过并算通过 —— 这让同一个
/// `/init` 在带网卡与不带网卡的测试里都能跑（不把「没有网卡」当失败）。
fn socket_selftest() -> bool {
    let mut ok = true;

    // 0) ABI 负例（不需要网卡）：地址族/类型/协议、以及「fd 不是套接字」。
    let bad_family = socket(AF_UNSPEC, SOCK_DGRAM, 0);
    println!("hello: socket bad-family ret={}", bad_family);
    ok &= bad_family == EAFNOSUPPORT; // -97
    let bad_type = socket(AF_INET, SOCK_STREAM, 0);
    println!("hello: socket stream ret={}", bad_type);
    ok &= bad_type == EPROTONOSUPPORT; // -93

    // 一个普通文件 fd 传给 `bind`：必须是 `-ENOTSOCK`（而不是被当成套接字）
    let file_fd = open(MOTD_PATH, O_RDONLY);
    if file_fd >= 3 {
        let notsock = bind(file_fd as i32, &SockAddrIn::any(0));
        println!("hello: bind on file fd ret={}", notsock);
        ok &= notsock == ENOTSOCK; // -88
        let _ = close(file_fd as i32);
    }

    // 未 `bind` 的套接字 `recvfrom`：`-EINVAL`（不知道收哪个端口）
    let unbound = socket(AF_INET, SOCK_DGRAM, 0);
    if unbound >= 3 {
        let mut scratch = [0u8; 8];
        let idle = recvfrom(unbound as i32, &mut scratch, None);
        println!("hello: recvfrom unbound ret={}", idle);
        ok &= idle == EINVAL; // -22
        let _ = close(unbound as i32);
    }

    // 1) 拿到内核协商到的本机地址/网关/MAC（没有网卡/未 DHCP → ENODEV，跳过网络部分）。
    let mut cfg = NetConfig::new();
    let n = net_config(&mut cfg);
    if n == ENODEV {
        println!("hello: socket skip network (no network device)");
        println!("hello: socket PASS={}", ok as u8);
        return ok;
    }
    println!("hello: net_config ret={}", n);
    ok &= n > 0;
    let mac = cfg.mac();
    println!(
        "hello: net ip={}.{}.{}.{} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        cfg.ip()[0], cfg.ip()[1], cfg.ip()[2], cfg.ip()[3],
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );

    // 2) 建一个 UDP 套接字
    let fd = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
    println!("hello: socket fd={}", fd);
    if fd < 3 {
        return false;
    }
    let fd = fd as i32;

    // 3) 绑到 DHCP 客户端端口 68（与内核网络自测一致：OFFER 会回到 68）
    let local = SockAddrIn::any(DHCP_CLIENT_PORT);
    let bound = bind(fd, &local);
    println!("hello: bind ret={}", bound);
    ok &= bound == 0;
    // 同一端口再绑一次 → `-EADDRINUSE`
    let clash = socket(AF_INET, SOCK_DGRAM, 0);
    if clash >= 3 {
        let dup = bind(clash as i32, &SockAddrIn::any(DHCP_CLIENT_PORT));
        println!("hello: bind in-use ret={}", dup);
        ok &= dup == EADDRINUSE; // -98
        let _ = close(clash as i32);
    }

    // 4) 拼一条 DHCP DISCOVER 并发给广播地址:67
    let mut request = [0u8; DHCP_LEN];
    build_discover(&mut request, mac);
    let dst = SockAddrIn::new(INADDR_BROADCAST, DHCP_SERVER_PORT);
    let sent = sendto(fd, &request, &dst);
    println!("hello: sendto ret={}", sent);
    ok &= sent == DHCP_LEN as isize;

    // 5) 收 OFFER，并校验发送方端口与 BOOTP 内容
    let mut reply = [0u8; 512];
    let mut src = SockAddrIn::empty();
    let got = recvfrom(fd, &mut reply, Some(&mut src));
    println!(
        "hello: recvfrom ret={} src={}.{}.{}.{}:{}",
        got, src.addr()[0], src.addr()[1], src.addr()[2], src.addr()[3], src.port()
    );
    ok &= got > 0 && src.port() == DHCP_SERVER_PORT;
    if got > 0 {
        let reply_len = got as usize;
        // BOOTP：op=2（reply）、magic cookie、yiaddr 非 0 ⇒ 真的是一张 OFFER
        let is_reply = reply[0] == 2;
        let cookie = reply_len >= 240 && reply[236..240] == [0x63, 0x82, 0x53, 0x63];
        let yiaddr = [reply[16], reply[17], reply[18], reply[19]];
        let assigned = yiaddr != [0, 0, 0, 0];
        println!(
            "hello: socket dhcp op={} cookie={} yiaddr={}.{}.{}.{}",
            reply[0], cookie as u8, yiaddr[0], yiaddr[1], yiaddr[2], yiaddr[3]
        );
        ok &= is_reply && cookie && assigned;
    }

    // 6) 关掉套接字：内核必须回收套接字槽位（fd 也回到空闲）
    let closed = close(fd);
    println!("hello: socket close ret={}", closed);
    ok &= closed == 0;

    println!("hello: socket PASS={}", ok as u8);
    ok
}

/// DHCP 服务器端口。
const DHCP_SERVER_PORT: u16 = 67;
/// DHCP 客户端端口（OFFER 的落点）。
const DHCP_CLIENT_PORT: u16 = 68;
/// DHCP 报文长度（RFC 2131 的最小报文：236 头 + 4 cookie + 选项 + 填充）。
const DHCP_LEN: usize = 300;

/// 拼一条 DHCP DISCOVER（BOOTP + options），与内核 `network/dhcp.rs` 同一种格式。
///
/// 固定请求**广播应答**（flags bit15）：OFFER 以广播帧回来，套接字一定能收到。
/// `xid` 用一个固定值即可（一次启动只发一条）。
fn build_discover(buf: &mut [u8; DHCP_LEN], mac: [u8; 6]) {
    buf.fill(0);
    buf[0] = 1; // op = BOOTREQUEST
    buf[1] = 1; // htype = Ethernet
    buf[2] = 6; // hlen
    buf[4..8].copy_from_slice(&0x4345_524Cu32.to_be_bytes()); // xid = "CERL"
    buf[10..12].copy_from_slice(&0x8000u16.to_be_bytes()); // 请求广播应答
    buf[28..34].copy_from_slice(&mac); // chaddr
    buf[236..240].copy_from_slice(&[0x63, 0x82, 0x53, 0x63]); // magic cookie
    buf[240] = 53; // option: DHCP message type
    buf[241] = 1;
    buf[242] = 1; // DISCOVER
    buf[243] = 55; // option: parameter request list
    buf[244] = 4;
    buf[245] = 1; // subnet mask
    buf[246] = 3; // router
    buf[247] = 6; // DNS
    buf[248] = 54; // server identifier
    buf[249] = 255; // END
}

/// CKM-1 自测：把「能力判定」在 ring3 上的结果留成可断言的证据。
///
/// `/init` 的固定身份是 **uid 0 + [`CAP_FS_SYSTEM_WRITE`]**（`docs/ckm.md` 3.1），
/// 所以每个期望值都是确定的，而不是「跑起来看看」：
///
/// | 操作 | 需要 | /init 的结果 |
/// | ---- | ---- | ---- |
/// | `fs_write`（受保护前缀） | `CAP_FS_SYSTEM_WRITE` | 允许（且有端到端写入） |
/// | `state_write` | `CAP_CKM_SERVICE` | `-EPERM`（需要 `0x2`） |
/// | `audit_read` | `CAP_AUDIT_READ` | `-EPERM` |
/// | `storage_rescan` | `CAP_STORAGE_ADMIN` | `-EPERM` |
/// | `policy_query` / `component_state` | 无 | 允许（只读） |
///
/// 最后一条是刻意加的**最小权限对照**：`/init` 能建立系统视图（写 `/etc`），
/// 但不能读审计、不能改策略、不能动存储 —— 这正是「不给 CKM 管理能力」的含义。
fn ckm_selftest() -> bool {
    let mut ok = true;
    let mut rec = [0u8; AUDIT_RECORD_SIZE];
    println!("hello: ckm selftest begin");

    // 1) 查询接口本身无能力要求（它只能问自己，不能枚举别人）
    let q = ckm_policy_query(CKM_OP_POLICY_QUERY, 0, &mut rec);
    println!(
        "hello: ckm policy_query(policy_query) -> {} need=0x{:x}",
        q,
        ckm_record_need(&rec, 0)
    );
    ok &= q == 0 && !ckm_record_is_deny(&rec, 0);

    // 2) 写受保护前缀：/init 持有 CAP_FS_SYSTEM_WRITE → 允许
    let q = ckm_policy_query(CKM_OP_FS_WRITE, CKM_PREFIX_ETC, &mut rec);
    println!(
        "hello: ckm policy_query(fs_write,etc) -> {} need=0x{:x}",
        q,
        ckm_record_need(&rec, 0)
    );
    ok &= q == 0;

    // 3) 写扩展状态树：需要 CAP_CKM_SERVICE，/init 没有 → 拒绝
    let q = ckm_policy_query(CKM_OP_STATE_WRITE, 0, &mut rec);
    println!(
        "hello: ckm policy_query(state_write) -> {} need=0x{:x}",
        q,
        ckm_record_need(&rec, 0)
    );
    ok &= q == EPERM && ckm_record_need(&rec, 0) == CAP_CKM_SERVICE;

    // 4) 审计读取需要 CAP_AUDIT_READ：/init 没有 → -1（审计环因此不是谁都能读的）
    let mut audit = [0u8; AUDIT_RECORD_SIZE * 4];
    let mut audit_total = 0u64;
    let n = ckm_audit_read(&mut audit, 0, Some(&mut audit_total));
    println!("hello: ckm audit_read -> {}", n);
    ok &= n == EPERM;

    // 5) 组件状态表：只读、无能力要求 → 允许（枚举顺序由内核保证确定性）
    let mut comps = [0u8; COMPONENT_RECORD_SIZE * 4];
    let mut comp_count = 0u64;
    let n = ckm_component_state(&mut comps, Some(&mut comp_count));
    println!("hello: ckm component_state -> {} count={}", n, comp_count);
    ok &= n == 3 && comp_count == 3;
    for i in 0..n as usize {
        println!(
            "hello: ckm component[{}] id={} kind={} state={}",
            i,
            component_id(&comps, i),
            component_kind_name(component_kind(&comps, i)),
            component_state_name(component_state(&comps, i))
        );
    }

    // 6) 存储管理（重扫设备）需要 CAP_STORAGE_ADMIN：/init 也不持有 → -1
    let rescan = storage_rescan(STORAGE_RESCAN_ALL);
    println!("hello: ckm storage_rescan -> {}", rescan);
    ok &= rescan == EPERM;

    // 7) 端到端允许：能力位不是「只在查询里返回 0」—— /init 真的写进了 /etc
    let fd = open(INIT_FLAG_PATH, O_CREAT | O_WRONLY);
    println!("hello: ckm open {} fd={}", INIT_FLAG_PATH, fd);
    ok &= fd >= 3;
    if fd >= 3 {
        let msg = b"init: system view established\n";
        let written = write(fd as i32, msg);
        println!("hello: ckm wrote {} bytes to {}", written, INIT_FLAG_PATH);
        let _ = close(fd as i32);
    }

    println!("hello: ckm selftest ok={}", ok as u8);
    ok
}

/// 用户堆自测：`brk` 增长、`mmap` 匿名映射、`munmap` 归还、越界/非法参数的负例，
/// 以及「全局分配器真的能用」（`Vec` + `format!`）。
fn heap_selftest() -> bool {
    let mut ok = true;

    // --- brk：查询 → 增长两页 → 新页必须是零 → 写入模式并读回 ---
    let base = brk(0);
    println!("hello: brk base={:#x}", base);
    let grown = brk(base + HEAP_STEP);
    println!("hello: brk grown={:#x}", grown);
    if grown != base + HEAP_STEP {
        return false;
    }
    let heap = base as *mut u8;
    let zeroed = unsafe {
        core::ptr::read_volatile(heap).eq(&0)
            && core::ptr::read_volatile(heap.add(4095)).eq(&0)
            && core::ptr::read_volatile(heap.add(8191)).eq(&0)
    };
    println!("hello: heap zeroed={}", zeroed as u8);
    ok &= zeroed;
    for i in 0..1024usize {
        unsafe { core::ptr::write_volatile(heap.add(i), (i & 0xff) as u8) };
    }
    let mut pattern_ok = true;
    for i in 0..1024usize {
        if unsafe { core::ptr::read_volatile(heap.add(i)) } != (i & 0xff) as u8 {
            pattern_ok = false;
        }
    }
    println!("hello: heap pattern={}", pattern_ok as u8);
    ok &= pattern_ok;

    // 越出堆窗口（256 KiB）：Linux 语义 → 返回**原值**，调用方据此判断失败
    let over = brk(base + 0x10_0000);
    let refused = over == grown;
    println!("hello: brk over ret={:#x} refused={}", over, refused as u8);
    ok &= refused;

    // --- mmap：匿名两页 → 零 → 模式 → 交给内核写串口 → munmap 归还 ---
    let m = mmap(MMAP_LEN, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS);
    println!("hello: mmap ret={}", m);
    if m <= 0 {
        return false;
    }
    let map = m as usize as *mut u8;
    let map_zeroed = unsafe {
        core::ptr::read_volatile(map).eq(&0) && core::ptr::read_volatile(map.add(MMAP_LEN - 1)).eq(&0)
    };
    println!("hello: mmap zeroed={}", map_zeroed as u8);
    ok &= map_zeroed;
    for i in 0..MMAP_LEN {
        unsafe { core::ptr::write_volatile(map.add(i), (i ^ 0xa5) as u8) };
    }
    let mut map_ok = true;
    for i in 0..MMAP_LEN {
        if unsafe { core::ptr::read_volatile(map.add(i)) } != (i ^ 0xa5) as u8 {
            map_ok = false;
        }
    }
    println!("hello: mmap pattern={}", map_ok as u8);
    ok &= map_ok;

    // 把**堆上的**缓冲区交给内核写出去：证明 `write` 的缓冲校验（按页查 `page_flags`）
    // 认可用户堆页，而不是只认映像窗口与栈。
    let msg = b"heap buffer through write\n";
    unsafe { core::ptr::copy_nonoverlapping(msg.as_ptr(), map, msg.len()) };
    let n = write(1, unsafe { core::slice::from_raw_parts(map, msg.len()) });
    println!("hello: heap write ret={}", n);
    ok &= n == msg.len() as isize;

    let un = munmap(m as u64, MMAP_LEN);
    println!("hello: munmap ret={}", un);
    ok &= un == 0;

    // --- 负例：每个错误码都要具体（不是「都失败就行」） ---
    // 1) 无 MAP_ANONYMOUS 但也没有有效 fd（libc 的 `mmap` 固定传 fd=-1）→ EINVAL(-22)
    let file_backed = mmap(4096, PROT_READ, MAP_PRIVATE);
    println!("hello: mmap file-backed ret={}", file_backed);
    ok &= file_backed == -22;
    // 2) MAP_SHARED 不支持 → EINVAL(-22)
    let shared = mmap(4096, PROT_READ, 0x1 | MAP_ANONYMOUS);
    println!("hello: mmap shared ret={}", shared);
    ok &= shared == -22;
    // 3) 超过 mmap 窗口 → ENOMEM(-12)
    let too_big = mmap(MMAP_MAX + 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS);
    println!("hello: mmap too big ret={}", too_big);
    ok &= too_big == -12;
    // 4) W^X：可写 + 可执行 → EINVAL(-22)
    let wx = mmap(4096, PROT_WRITE | 0x4, MAP_PRIVATE | MAP_ANONYMOUS);
    println!("hello: mmap wx ret={}", wx);
    ok &= wx == -22;
    // 5) 匿名映射带非 0 `offset`（第 6 参数 `r9`；文件映射未实现）→ EINVAL(-22)
    let offset = mmap_with_offset(4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, 4096);
    println!("hello: mmap offset ret={}", offset);
    ok &= offset == -22;
    // 6) `offset=0` 仍然成功（第 6 参数不破坏匿名映射）
    let zero_offset = mmap_with_offset(4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, 0);
    println!("hello: mmap zero-offset ret={}", zero_offset);
    ok &= zero_offset > 0;
    if zero_offset > 0 {
        ok &= munmap(zero_offset as u64, 4096) == 0;
    }
    // 7) munmap 未页对齐 → EINVAL(-22)
    let misaligned = munmap(m as u64 + 1, MMAP_LEN);
    println!("hello: munmap misaligned ret={}", misaligned);
    ok &= misaligned == -22;
    // 6) 同一段再次 munmap（已经不映射）→ EINVAL(-22)
    let twice = munmap(m as u64, MMAP_LEN);
    println!("hello: munmap twice ret={}", twice);
    ok &= twice == -22;

    // --- 全局分配器：`Vec` 增长（多次分配/重新分配）+ `format!`（String） ---
    let mut v: Vec<u32> = Vec::new();
    for i in 1..=200u32 {
        v.push(i);
    }
    let sum: u32 = v.iter().sum();
    println!("hello: vec len={} sum={}", v.len(), sum);
    ok &= v.len() == 200 && sum == 20100;
    let s = alloc::format!("heap string bytes={}", v.len());
    println!("hello: {}", s);
    ok &= s.len() == 21;

    println!("hello: heap PASS={}", ok as u8);
    ok
}

/// 文件映射自测（v1.0 第三步）：`mmap_file(fd, len, PROT_READ, MAP_PRIVATE, offset)`
/// 把文件的 `[offset, offset+len)` 只读映射进本进程，验证内容与 `offset` 真的生效，
/// 以及三类拒绝（非页对齐 `offset` / 写权限 / `MAP_SHARED`）。
fn file_mmap_selftest() -> bool {
    let mut ok = true;

    // 1) 造一个已知模式的文件（5120 字节，`byte[i] = i & 0xff`）
    let fd = open(MMAP_FILE_PATH, O_CREAT | O_WRONLY);
    println!("hello: filemmap create {} fd={}", MMAP_FILE_PATH, fd);
    if fd < 0 {
        return false;
    }
    let mut buf = [0u8; 512];
    let mut written = 0usize;
    while written < MMAP_FILE_LEN {
        for i in 0..buf.len() {
            buf[i] = ((written + i) & 0xff) as u8;
        }
        let n = write(fd as i32, &buf);
        if n <= 0 {
            break;
        }
        written += n as usize;
    }
    let _ = close(fd as i32);
    println!("hello: filemmap wrote={}", written);
    ok &= written == MMAP_FILE_LEN;

    // 2) 只读打开，映射 offset = 4096 处的一页
    let rfd = open(MMAP_FILE_PATH, O_RDONLY);
    println!("hello: filemmap open ro fd={}", rfd);
    if rfd < 0 {
        return false;
    }
    let m = mmap_file(rfd as i32, 4096, PROT_READ, MAP_PRIVATE, 4096);
    println!("hello: filemmap mmap ret={}", m);
    ok &= m > 0;
    if m > 0 {
        let p = m as usize as *const u8;
        let mut good = true;
        for i in 0..1024usize {
            let want = ((4096 + i) & 0xff) as u8;
            if unsafe { core::ptr::read_volatile(p.add(i)) } != want {
                good = false;
            }
        }
        println!("hello: filemmap contents={}", good as u8);
        ok &= good;
        let un = munmap(m as u64, 4096);
        println!("hello: filemmap munmap ret={}", un);
        ok &= un == 0;
    }

    // 3) 负例：每一类拒绝都要具体
    let misaligned = mmap_file(rfd as i32, 4096, PROT_READ, MAP_PRIVATE, 1);
    println!("hello: filemmap misaligned ret={}", misaligned);
    ok &= misaligned == -22; // offset 未页对齐
    let exec = mmap_file(rfd as i32, 4096, PROT_READ | PROT_EXEC, MAP_PRIVATE, 0);
    println!("hello: filemmap exec ret={}", exec);
    ok &= exec == -22; // 文件映射不可执行
    let shared_ro = mmap_file(rfd as i32, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, 0);
    println!("hello: filemmap shared ro ret={}", shared_ro);
    ok &= shared_ro == -9; // MAP_SHARED|PROT_WRITE 需要一个可写打开的 fd（EBADF）
    let _ = close(rfd as i32);

    // 4) `MAP_PRIVATE` + 可写（v1.0 第五步）：内容是文件的副本，**改写不落盘**
    let rwfd = open(MMAP_FILE_PATH, O_RDWR);
    println!("hello: filemmap open rw fd={}", rwfd);
    if rwfd < 0 {
        return false;
    }
    let pm = mmap_file(rwfd as i32, 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE, 0);
    println!("hello: filemmap private mmap ret={}", pm);
    ok &= pm > 0;
    if pm > 0 {
        unsafe { core::ptr::write_volatile(pm as usize as *mut u8, 0x77) };
        let un = munmap(pm as u64, 4096);
        println!("hello: filemmap private munmap ret={}", un);
        ok &= un == 0;
        let mut one = [0u8; 1];
        let n = read_head(MMAP_FILE_PATH, &mut one);
        // 文件的第 0 字节是模式里的 0，`MAP_PRIVATE` 的改写不该出现在这里
        let kept = n == 1 && one[0] == 0;
        println!("hello: filemmap private kept={}", kept as u8);
        ok &= kept;
    }
    let _ = close(rwfd as i32);

    println!("hello: filemmap PASS={}", ok as u8);
    ok
}

/// 读文件**头部** `buf.len()` 字节，返回实际读到的字节数（出错返回 `-1`）。
///
/// 每次新开一个 fd：映射自测里别的 fd 的偏移不该被这个检查扰动。
fn read_head(path: &str, buf: &mut [u8]) -> i32 {
    let fd = open(path, O_RDONLY);
    if fd < 0 {
        return -1;
    }
    let mut done = 0usize;
    while done < buf.len() {
        let n = read(fd as i32, &mut buf[done..]);
        if n <= 0 {
            break;
        }
        done += n as usize;
    }
    let _ = close(fd as i32);
    done as i32
}

/// 共享文件映射自测（v1.0 第五步）：`MAP_SHARED` 的改动要**真的落盘**。
///
/// 三件可断言的事实：
///
/// 1. `msync` 触发写回，回读文件能看到改动（`user: msync … -> pages=1 bytes=2048`）；
/// 2. 不 `msync`、直接 `munmap` 也写回（归还之前内核自动回写一次）；
/// 3. 写回被**夹在文件大小内**：映射 4 KiB 而文件 2 KiB，映射尾部那一页（填充零、
///    或用户写进去的字节）不会把文件撑大 —— `hello: sharedmmap persisted=1 filelen=2048`。
///
/// 另加四类负例：文件映射不可执行（`-22`）、只读 fd 上的 `MAP_SHARED|PROT_WRITE`
/// （`-9`，在 `file_mmap_selftest` 里）、共享映射的**部分** `munmap`（`-22`，
/// 描述符按整段登记，不做 VMA 分裂）、`msync` 既没有 `MS_SYNC` 也没有 `MS_ASYNC`（`-22`）。
fn shared_mmap_selftest() -> bool {
    let mut ok = true;

    // 1) 造一个 2048 字节的模式文件（byte[i] = i & 0xff）
    let fd = open(SHARED_FILE_PATH, O_CREAT | O_WRONLY);
    println!("hello: sharedmmap create {} fd={}", SHARED_FILE_PATH, fd);
    if fd < 0 {
        return false;
    }
    let mut buf = [0u8; 512];
    let mut written = 0usize;
    while written < SHARED_FILE_LEN {
        for i in 0..buf.len() {
            buf[i] = ((written + i) & 0xff) as u8;
        }
        let n = write(fd as i32, &buf);
        if n <= 0 {
            break;
        }
        written += n as usize;
    }
    let _ = close(fd as i32);
    println!("hello: sharedmmap wrote={}", written);
    ok &= written == SHARED_FILE_LEN;

    // 2) 读写打开 + MAP_SHARED（一页 = 4 KiB，比文件长 2 KiB）
    let rwfd = open(SHARED_FILE_PATH, O_RDWR);
    println!("hello: sharedmmap open rw fd={}", rwfd);
    if rwfd < 0 {
        return false;
    }
    let m = mmap_file(rwfd as i32, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, 0);
    println!("hello: sharedmmap mmap ret={}", m);
    if m <= 0 {
        let _ = close(rwfd as i32);
        return false;
    }

    // 3) 映射内容 = 文件内容，且文件尾之后是零
    let p = m as usize as *mut u8;
    let mut good = true;
    for i in 0..SHARED_FILE_LEN {
        if unsafe { core::ptr::read_volatile(p.add(i)) } != ((i & 0xff) as u8) {
            good = false;
        }
    }
    let tail = unsafe { core::ptr::read_volatile(p.add(SHARED_FILE_LEN)) };
    println!(
        "hello: sharedmmap contents={} tail0={}",
        good as u8,
        (tail == 0) as u8
    );
    ok &= good && tail == 0;

    // 4) ring3 改三处：首字节、第 100 字节、文件最后一字节；
    //    再往**文件尾之后**写一个字节 —— 它必须留在内存里（不落盘）
    unsafe {
        core::ptr::write_volatile(p, 0xA5);
        core::ptr::write_volatile(p.add(100), 0x5A);
        core::ptr::write_volatile(p.add(SHARED_FILE_LEN - 1), 0x11);
        core::ptr::write_volatile(p.add(SHARED_FILE_LEN), 0xFF);
    }

    // 5) `msync` 显式写回
    let ms = msync(m as u64, 4096, MS_SYNC);
    println!("hello: sharedmmap msync ret={}", ms);
    ok &= ms == 0;

    // 6) 回读文件：三处改动都在、未改动的字节原样、文件长度仍是 2048
    let mut head = [0u8; SHARED_FILE_LEN];
    let got = read_head(SHARED_FILE_PATH, &mut head);
    let persisted = got == SHARED_FILE_LEN as i32
        && head[0] == 0xA5
        && head[1] == 1
        && head[100] == 0x5A
        && head[SHARED_FILE_LEN - 1] == 0x11;
    println!(
        "hello: sharedmmap persisted={} filelen={}",
        persisted as u8, got
    );
    ok &= persisted;

    // 7) `munmap` 归还
    let un = munmap(m as u64, 4096);
    println!("hello: sharedmmap munmap ret={}", un);
    ok &= un == 0;

    // 8) 第二遍：不 `msync`，改完直接 `munmap` —— 归还前内核自动写回
    let m2 = mmap_file(rwfd as i32, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, 0);
    println!("hello: sharedmmap remap ret={}", m2);
    ok &= m2 > 0;
    if m2 > 0 {
        // `msync` 的负例：只给 MS_INVALIDATE（既没有 MS_SYNC 也没有 MS_ASYNC）→ -22
        let noflag = msync(m2 as u64, 4096, MS_INVALIDATE);
        println!("hello: sharedmmap msync noflag ret={}", noflag);
        ok &= noflag == -22;
        unsafe { core::ptr::write_volatile(m2 as usize as *mut u8, 0x3C) };
        let un2 = munmap(m2 as u64, 4096);
        println!("hello: sharedmmap munmap2 ret={}", un2);
        ok &= un2 == 0;
        let mut one = [0u8; 1];
        let n = read_head(SHARED_FILE_PATH, &mut one);
        let persisted2 = n == 1 && one[0] == 0x3C;
        println!("hello: sharedmmap munmap persisted={}", persisted2 as u8);
        ok &= persisted2;
    }

    // 9) 负例：共享文件映射必须**整段**归还（两页映射只归还一页 → -22）
    let partial = mmap_file(rwfd as i32, 8192, PROT_READ | PROT_WRITE, MAP_SHARED, 0);
    println!("hello: sharedmmap partial mmap ret={}", partial);
    ok &= partial > 0;
    if partial > 0 {
        let half = munmap(partial as u64, 4096);
        println!("hello: sharedmmap partial munmap ret={}", half);
        ok &= half == -22;
        // 被拒的归还**没有**动到映射：这一页还能写
        unsafe { core::ptr::write_volatile(partial as usize as *mut u8, 0x5C) };
        let full = munmap(partial as u64, 8192);
        println!("hello: sharedmmap partial cleanup ret={}", full);
        ok &= full == 0;
    }

    // 10) 匿名映射上 `msync` 是合法但空操作（`pages=0 bytes=0`）
    let anon = mmap(4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS);
    println!("hello: sharedmmap anon mmap ret={}", anon);
    ok &= anon > 0;
    if anon > 0 {
        let s = msync(anon as u64, 4096, MS_SYNC);
        println!("hello: sharedmmap anon msync ret={}", s);
        ok &= s == 0;
        let un3 = munmap(anon as u64, 4096);
        println!("hello: sharedmmap anon munmap ret={}", un3);
        ok &= un3 == 0;
    }

    // 11) 窗口外的地址 → -22（页对齐、窗口、全段已映射三者缺一不可）
    let bad = msync(0x0000_8000_0000_0000, 4096, MS_SYNC);
    println!("hello: sharedmmap msync bad addr ret={}", bad);
    ok &= bad == -22;

    let _ = close(rwfd as i32);
    println!("hello: sharedmmap PASS={}", ok as u8);
    ok
}

/// `mprotect` 自测（v1.0 第一步）：`PROT_NONE` 关页 → 内核拒收该缓冲 → 恢复后内容原样
/// → 换成 `RX` 并**真的执行**页里的机器码（W^X 的「先写后换只执行」）。
///
/// 每一类拒绝都给**具体**退出码（与 `mmap`/`munmap` 的负例同一风格）：
/// 未页对齐 / `len=0` / 未知保护位 / `W^X` / 未映射的页 / 用户栈（保护属性由内核定死）。
fn mprotect_selftest() -> bool {
    let mut ok = true;

    // 一页 RW：写下机器码 —— 此刻这页还不能执行
    let m = mmap(4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS);
    println!("hello: mprotect mmap ret={}", m);
    if m <= 0 {
        return false;
    }
    let page = m as usize;
    let ptr = page as *mut u8;
    unsafe { core::ptr::copy_nonoverlapping(WX_CODE.as_ptr(), ptr, WX_CODE.len()) };

    // 1) PROT_NONE：页帧与内容都还在，但本进程不可访问；内核也不再把这段当合法用户缓冲
    //    （用 `raw` 直接发 syscall，避免在不可读的页上构造 Rust 切片）
    let none = mprotect(page as u64, 4096, PROT_NONE);
    println!("hello: mprotect none ret={}", none);
    ok &= none == 0;
    let refused = unsafe { raw(SYS_WRITE, 1, page as u64, 1, 0, 0) };
    println!("hello: mprotect none write ret={}", refused);
    ok &= refused == -14; // EFAULT：按页校验认定为「不是用户缓冲」

    // 2) 改回 RW：内容必须原样（mprotect 不搬家、不清零）
    let back = mprotect(page as u64, 4096, PROT_READ | PROT_WRITE);
    println!("hello: mprotect rw ret={}", back);
    ok &= back == 0;
    let kept = unsafe { core::ptr::read_volatile(ptr) } == WX_CODE[0];
    println!("hello: mprotect data kept={}", kept as u8);
    ok &= kept;

    // 3) 换成 RX 并真的调用：JIT 走的就是这条路
    let rx = mprotect(page as u64, 4096, PROT_READ | PROT_EXEC);
    println!("hello: mprotect rx ret={}", rx);
    ok &= rx == 0;
    let entry: extern "C" fn() -> u32 = unsafe { core::mem::transmute(page) };
    let ret = entry();
    println!("hello: mprotect exec ret={}", ret);
    ok &= ret == 42;

    // 4) 负例：每一类拒绝都要具体
    let misaligned = mprotect(page as u64 + 8, 4096, PROT_READ);
    println!("hello: mprotect misaligned ret={}", misaligned);
    ok &= misaligned == -22;
    let len0 = mprotect(page as u64, 0, PROT_READ);
    println!("hello: mprotect len0 ret={}", len0);
    ok &= len0 == -22;
    let unknown = mprotect(page as u64, 4096, 0x8);
    println!("hello: mprotect unknown prot ret={}", unknown);
    ok &= unknown == -22;
    let wx = mprotect(page as u64, 4096, PROT_WRITE | PROT_EXEC);
    println!("hello: mprotect wx ret={}", wx);
    ok &= wx == -22;
    // 邻页没有映射（本程序只映射了这一页）→ 不做事，而不是「部分成功」
    let unmapped = mprotect(page as u64 + 4096, 4096, PROT_READ);
    println!("hello: mprotect unmapped ret={}", unmapped);
    ok &= unmapped == -22;
    // 用户栈：保护属性（RW + NX）由内核在建立地址空间时定死，不在支持范围
    let local = 0u8;
    let stack_page = (&local as *const u8 as u64) & !0xfff;
    let stack = mprotect(stack_page, 4096, PROT_READ);
    println!("hello: mprotect stack ret={}", stack);
    ok &= stack == -22;

    // 5) brk 堆窗口也在支持范围内（逐页标志位 `heap_flags` 的意义）——
    //    heap_selftest 增长了两页，所以当前 break 减 HEAP_STEP 就是堆的第一页
    let heap_page = brk(0) - HEAP_STEP;
    let heap_none = mprotect(heap_page, 4096, PROT_NONE);
    println!("hello: mprotect heap none ret={}", heap_none);
    ok &= heap_none == 0;
    let heap_refused = unsafe { raw(SYS_WRITE, 1, heap_page, 1, 0, 0) };
    println!("hello: mprotect heap none write ret={}", heap_refused);
    ok &= heap_refused == -14;
    let heap_back = mprotect(heap_page, 4096, PROT_READ | PROT_WRITE);
    println!("hello: mprotect heap back ret={}", heap_back);
    ok &= heap_back == 0;
    // 堆里第一页的第一个字节是 heap_selftest 写的 `0 & 0xff`
    let heap_kept = unsafe { core::ptr::read_volatile(heap_page as *const u8) } == 0;
    println!("hello: mprotect heap data kept={}", heap_kept as u8);
    ok &= heap_kept;

    // 归还：`mprotect` 过（含 PROT_NONE）的页帧仍在，munmap 必须成功
    let un = munmap(page as u64, 4096);
    println!("hello: mprotect munmap ret={}", un);
    ok &= un == 0;

    println!("hello: mprotect PASS={}", ok as u8);
    ok
}
