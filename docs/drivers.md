# 设备驱动 / PCI / 磁盘（阶段文档）

- 阶段：第八阶段（v0.8 驱动框架 / PCI / 块设备；v0.9 网络栈；v0.4 中断控制时钟键盘已完成）
- 最后更新：2026-10-11
- 状态：✅ v0.8 已完成（**驱动注册框架 + PCI 枚举 + 块设备抽象 + virtio-blk（legacy）**，`make test-driver` 22 项 PASS；**磁盘文件系统 FAT12/16/32 已挂到 VFS**，`make test-fat` 21 项 PASS）；**v0.9 简化网络栈已落地**（**virtio-net（legacy）+ Ethernet → ARP → IPv4 → UDP**，`make test-network` 13 项 PASS）；**ICMP Echo（ping）已落地**；**v1.0 第四步：UDP 套接字最小集已落地**（`socket`/`bind`/`sendto`/`recvfrom` + `net_config`，`make test-network` 增至 30 项；**TCP 仍为占位**，见 1.8/1.9）；v0.4（中断控制器 + PIT + PS/2 键盘 + 上下文切换原语 + H-04/H-05 Go 工具）已完成
- 依赖：interrupt（IRQ 分发）、memory（连续物理帧 → DMA / virtqueue）、arch::port（端口 I/O）

> 本文档顶部保留阶段/最后更新/状态字段，与项目文档维护约定对齐。

## 1. 当前状态（2026-10-11）

### 1.1 v0.8 已落地：驱动框架 + PCI 枚举 + 块设备

代码位置与职责：

| 文件 | 职责 |
| ---- | ---- |
| `kernel/src/driver/pci.rs` | PCI 枚举（配置机制 #1：`0xCF8`/`0xCFC`）、BAR 探测与窗口大小计算、设备表、`print_bdf` |
| `kernel/src/driver/driver.rs` | 驱动注册表 + `Driver` trait（`name`/`matches`/`attach`）+ `probe_all()` 匹配与挂接 + `ProbeStats` |
| `kernel/src/driver/block.rs` | 块设备抽象 `BlockDevice`（按 512 字节扇区读写）+ 设备注册表 + 块设备验收自测 |
| `kernel/src/driver/virtio_blk.rs` | legacy virtio-blk 0.9.5 驱动：virtqueue 布局、描述符链、notify、used ring 轮询、bounce buffer |
| `kernel/src/arch/x86_64/port.rs` | 端口 I/O 统一入口（`inb/inw/inl` / `outb/outw/outl`） |
| `kernel/src/memory/frame.rs` | `allocate_contiguous(count)` / `free_contiguous(...)`：virtqueue 需要的**物理连续**帧 |

启动顺序（`kernel/src/main.rs`，在 `Kernel started!` 之后、调度器/用户态之前）：

```text
pci::init()            → 枚举 256×32 个 function，填设备表并逐设备打印一行
find_by_id(1af4,1001)   → 断言启动盘所在的 legacy virtio-blk 确实在总线上
driver::register_builtin() → 注册表里放入 virtio-blk-pci
driver::probe_all()    → 逐设备匹配（注册顺序优先）→ attach → 统计 matched/attached/failed
block::selftest(disk)  → 引导扇区 / 分块读 / 写回读 / 负例
fs::mount_disks()      → 把 FAT 卷挂到 /disk（启动盘）与 /mnt/fat（mkfs 测试盘，最长前缀路由）
fs::disk_selftest()    → 磁盘文件系统验收（读内核自己的映像 / 跨簇公式 / LFN / 偏移读 / EROFS）
```

最后两行是 v0.8 收官部分：磁盘的文件系统层（`kernel/src/fs/fat.rs`）与驱动层 1.1 表格里的其它模块
同属一个启动链 —— 驱动只是把扇区搬进内存，**FAT 与 VFS 才是「文件」的落点**，
详见 [filesystem.md](./filesystem.md) 第 3 节。

### 1.2 关键设计决策（为什么是这样）

**PCI 用配置机制 #1（`0xCF8`/`0xCFC`）而不是 ECAM。** 机制 #1 每次访问两条端口指令，不依赖 ACPI；
ECAM 需要先解析 MCFG（ACPI）表，那是引入 ACPI 之后才划算的优化。代价（约 8k 次端口读扫描全总线）
在启动日志里可忽略，换来的是「枚举」这条路径自成一体、可被单测形态的串口断言完全覆盖。

**只实现 legacy virtio（0.9.5），不做 modern（virtio 1.0）。** 设备 ID 决定接口：`0x1001`（legacy，
BAR0 是 I/O 窗口，寄存器是简单偏移）/ `0x1042`（modern，寄存器在 capability 指向的 MMIO 里，
队列布局也不同）。legacy 的实现量大约只有现代接口的三分之一，而 QEMU 用
`disable-modern=on` 就能稳定提供 legacy 设备；真遇到 modern 设备时 `attach` 会**打印明确原因并返回错误**
（不静默失败），错误会计入 `ProbeStats::failed`。

**块 I/O 走同步轮询，不用中断。** v0.8 不配置 MSI-X/INTx，每次请求提交后自旋等 used ring 的 `idx` 前进
（上限由 PIT tick 给出）。理由是：块 I/O 常在无法睡眠的路径上发生，而中断上下文里做块 I/O 是另一类
复杂度的开始。代价是等待期间忙等；v0.9 起再改为「中断 + 等待队列」。

**bounce buffer 换掉一整类 DMA 对齐问题。** 调用方缓冲长度只要整扇区对齐即可，驱动内部用一块
页对齐静态缓冲（8 扇区 = 4 KiB）分块搬运。因此不要求调用方缓冲物理连续、也不要求对齐；
一次性拷贝的成本换来「上层随便传缓冲」的可用性。

**驱动框架只做匹配与挂接，不做生命周期管理。** 注册表容量固定（`MAX_DRIVERS`），启动阶段写入、
此后只读；一个设备最多被一个驱动接管；`attach` 失败不阻断其它设备。上层（`main`）只需要看
`ProbeStats`，不需要知道总线上有多少种设备。

### 1.3 验收证据（`make test-driver`，22 项断言；以下是真实串口输出）

```text
pci: 00:03.0 vendor=0x1af4 device_id=0x1001 class=0x1 subclass=0x0 prog_if=0x0   ← legacy virtio-blk
pci: 00:03.0 bar0=io:0x6000 size=0x80                                            ← BAR0 是 I/O 窗口（legacy 的标志）
pci: enumerated devices=5
pci: virtio-blk (legacy) at 00:03.0 class=0x1 subclass=0x0
driver: registered index=0 name=virtio-blk-pci
driver: match 00:03.0 -> virtio-blk-pci
virtio-blk: legacy BAR0=0x6000 size=0x80
virtio-blk: host features=0x71006ed4
virtio-blk: guest features=0x0
virtio-blk: queue0 size=256 region=0x100000 (3 pages contiguous)
virtio-blk: capacity=131072 sectors (64 MiB)
virtio-blk: status=0xf (DRIVER_OK)
virtio-blk: registered as block device 0
driver: attached 00:03.0 -> virtio-blk-pci
driver: probe PASS (devices=5 matched=1 attached=1 failed=0)
block: registered devices=1
block: device=virtio-blk0 sector_size=512 sectors=131072 bytes=67108864
block: boot sector sig=0x55aa fstype=FAT16 oem=MTOO4032 sectors_per_cluster=2
block: chunked read PASS (512/4096/4608 bytes consistent at lba 0)
block: write/readback PASS (lba=131071 pattern=0xa5)
block: negative cases PASS (unaligned len, lba beyond capacity, overflow)
block: selftest PASS
```

这些断言的意义（「硬件事实」而不是日志措辞）：

- `device_id=0x1001` + `bar0=io:`：QEMU 在 `disable-modern=on` 下暴露的**就是** legacy 接口，
  与驱动实现的前提一致（modern 是 `0x1042`）；
- `queue0 size=256 region=0x100000 (3 pages contiguous)`：队列深度来自设备，队列区来自
  `frame::allocate_contiguous`（描述符表 + avail + 4 KiB 对齐的 used 三段必须物理连续，
  因为 legacy 的队列地址寄存器只装一个 PFN）；
- `capacity=131072 sectors (64 MiB)`：容量由设备报出，与 `tools/image-builder/build_image.py`
  的镜像尺寸互相印证；
- `boot sector sig=0x55aa fstype=FAT16`：从磁盘真的读回了本次构建的 FAT 引导扇区
  （类型串取自 BPB 的根目录项数决定的位置，不是写死的字符串）；
- `chunked read PASS`：4608 字节的读**跨过**驱动内部 8 扇区（4 KiB）搬运边界，与 4096 / 512
  两次独立读的拼接结果逐字节一致 → 分块重组与 LBA 偏移换算正确；
- `write/readback PASS (lba=131071 ...)`：最后一个扇区写 → 读回 → **还原** → 再读回；
  用非 snapshot 方式启动后比对镜像的 FAT 区域，确认「还原」真的把盘恢复原样（见 1.5）；
  > 该扇区自 CKM-2 起落在**状态区**（镜像尾部 1 MiB）的末尾：它仍然被还原，
  > 而状态区的两个槽在区域**开头**（`ckm/store.rs`），两者不重叠；
- `negative cases PASS`：长度非整扇区、LBA 越过容量、LBA 溢出（`u64::MAX-1`）三种输入都必须
  报错 —— 最后一条曾经真的漏过（`lba + sectors` 环绕后看起来在容量内），见 B-16/B-17。

### 1.4 已知限制（明确未做，不要读成已完成）

- **磁盘文件系统是只读的**：FAT12/16/32 后端已挂到 VFS（`fs/fat.rs`），但 `write`/`create`/`mkdir`
  一律返回 `EROFS` —— 分配器与目录项改写属后续版本；也没有扇区缓存（每次访问都真读设备）。
  详情与限制清单见 [filesystem.md](./filesystem.md) 3.1。
- **不分配/重定位 BAR**：直接使用固件分配的地址。真实平台上若出现未分配窗口（全 0）会打日志而不是猜测
  （v0.8 的实现里 `read_bar` 对全 0 返回「未实现」）。
- **不解析 capability 链、不配置中断**：MSI-X/INTx 与设备特定能力结构仍未做；v0.9 的网络驱动同样走
  **同步轮询**（不配置 MSI-X/INTx），因此这条限制对块设备与网卡都成立（见 1.8）。
- **最多两块 virtio-blk 盘**（`block` 注册表容量 2，每块盘各自的队列状态在实例里、请求缓冲共用）：
  启动盘 + 测试盘刚好够用，更多盘位留给后续版本。
- **不实现 modern（virtio 1.0）接口**：见 1.2；遇到 modern 设备时明确报错。
- **queue 只支持 0 号队列**：块设备只有一个 virtqueue，够用；多队列留给 v0.9+。
- 端口 I/O 的新统一入口（`arch/x86_64/port.rs`）只被 PCI / virtio 使用；早期驱动（serial/keyboard/PIC）
  仍各自保留私有副本 —— 改造它们只会扩大 diff，与 v0.8 目标无关。

### 1.5 磁盘写入的「无损」验证

`block: selftest` 会写最后一个扇区再做还原。为确认还原有效（而不只是「读回自身缓存」），
用**不带 `snapshot=on`** 的方式启动一次并比对镜像的 **FAT 区域（前 63 MiB）**：

```text
before=26a5e771b617734a3ac211caff0fa189
after =26a5e771b617734a3ac211caff0fa189
IMAGE UNCHANGED (write test restored the scratch sector)
```

> **CKM-2（2026-10-12）起只比前 63 MiB**：镜像尾部 1 MiB 是 **CKM 状态区**
> （`kernel/src/ckm/store.rs` 的双槽 A/B + CRC32），启动时**按设计会被写入**
> （首次开机建立状态区，之后 `put` 只写非活动槽）。这是两个区域的两条不同性质，不是「无损」被破坏：
> 扇区 131071（scratch）的内容被还原，状态区按原子写更新 —— 第二次启动读回 `state=loaded` 就是
> 「读到的是盘上的字节」的直接证据（`make test-ckm-store`）。同理，`make run` 之后整盘 md5 会变。

测试套件里则统一用 `snapshot=on`（临时写时复制），既让并行场景共享同一镜像，也不污染原始文件；
只有 CKM-2 的持久化自测（`tests/test_ckm_store.py` + `--persist`）会真的落盘，且它用的是**独占副本**。

### 1.6 v0.4 已落地部分（中断控制器 + PIT 100Hz + PS/2 键盘 + 上下文切换原语）

- [x] 中断控制器骨架与 PIC 第一版（初始化、IRQ→向量映射、mask/EOI 原语）
- [x] `kernel/src/interrupt/` 绑定为 `controller` / `exception` / `irq` 三子模块
- [x] `kernel_main` 中接入 PIC 初始化（默认全屏蔽，不使能外设 IRQ）
- [x] 选型策略记录：`kernel/src/interrupt/pic_policy.md`
- [x] PIT/Timer 子系统（`kernel/src/time/`：通道 0 重装载值、tick 计数）
- [x] IRQ 分发入口与 16 槽 IRQ 注册表（`kernel/src/interrupt/irq.rs`）
- [x] IRQ0（PIT）回调：100Hz，每 100 tick 打印一次 `IRQ0_heartbeat tick=<n>`
- [x] PS/2 键盘中断输入（`kernel/src/driver/keyboard.rs`）：8042 初始化、IRQ1、set 1 解码
- [x] **行缓冲与终端式回显（v0.9）**：128 字节行缓冲 + 行编辑（退格、回显），`Enter` 把整行标为可取
      并打印 `kbd: line ready len=N`；`stdin`（fd=0）由 `take_line` 取走（`syscall/file.rs::read_stdin`）。
      回显协议由 v0.4 的 `KB <字符>` 改为**原样回显**（理由：终端语义；`make test-keyboard` 断言同步改为
      `kbd: line ready len=3`，`KBIRQ ENTRY/EXIT` 不变）
- [x] 上下文切换原语（`kernel/src/arch/x86_64/context.rs`）：`switch_to` / `init_context` + 内核自测
- [x] `make test-keyboard`：QEMU monitor `sendkey` 注入真实按键并断言回显

### 1.7 中断/时钟侧的已知状态与限制

- PIC 映射固定为 master/slave + IRQ0..15 → 向量 32..47；APIC（LAPIC/IOAPIC/redirection）仍为占位。
- IRQ 汇编入口（`irq_common`）保存/恢复 15 个通用寄存器，**不保存** XMM/FPU 状态（内核不用浮点）。
- 键盘行缓冲是**单行**（一行的容量 128、含行尾 `\n`）：还没有输入队列，上一行没被 `read` 取走时
  新按键被丢弃（`driver/keyboard.rs::LineBuffer` 文档说明了这个互斥约定）。多行队列/终端抽象等 Shell 之后的控制台阶段。
- 回显在**中断上下文直接打印**（cooked 模式）：同一串口通道上还有 `KBIRQ ENTRY/EXIT` 标记，
  可读性靠「一次击键一段标记」的约定，终端抽象尚未引入。

### 1.8 v0.9 已落地：简化网络栈（virtio-net → Ethernet → ARP → IPv4 → UDP）

代码位置与职责：

| 文件 | 职责 |
| ---- | ---- |
| `kernel/src/driver/net.rs` | 网络设备抽象 `NetworkDevice`（以太网帧收发）+ 注册表 + MAC 打印 |
| `kernel/src/driver/virtio_net.rs` | legacy virtio-net 0.9.5：两个 virtqueue（0 = 接收 / 1 = 发送）、10 字节 virtio-net 头、MAC 读自设备配置空间、同步轮询 |
| `kernel/src/network/ethernet.rs` | Ethernet II 组帧 / 解析（纯函数，不依赖设备） |
| `kernel/src/network/arp.rs` | ARP 请求 / 应答 + 固定容量 IP→MAC 缓存 |
| `kernel/src/network/ipv4.rs` | IPv4 地址、头组帧 / 解析、互联网校验和（`sum16` / `finish`） |
| `kernel/src/network/udp.rs` | UDP 组帧 / 解析（发送算 IPv4 伪头校验和；接收校验非零校验和） |
| `kernel/src/network/dhcp.rs` | DHCP DISCOVER 组包 + OFFER 解析（UDP 往返的验收载体） |
| `kernel/src/network/mod.rs` | `Stack`（设备 + 地址 + ARP 缓存）+ `NetConfig`/`udp_send`/`udp_recv_poll` + `selftest` 验收编排 |
| `kernel/src/network/socket.rs` | 面向用户态的 UDP 套接字表（v1.0 第四步：本地端口 + 临时端口分配） |
| `kernel/src/syscall/socket.rs` | `socket`/`bind`/`sendto`/`recvfrom` + `net_config` 处理器（见 `docs/syscall.md` 3.14） |

启动顺序（挂载磁盘文件系统之后、调度器/用户态之前）：

```text
net::count()/first()      → 只有总线上挂了网卡才跑（无网卡时打印一行并跳过 —— 不把「没有网卡」当失败）
net::report()             → 打印设备名与 MAC
network::selftest(dev)    → ARP 解析网关 + DHCP DISCOVER → OFFER
```

**验收（`make test-network`，13 项串口断言）** —— QEMU 加一块 legacy virtio-net-pci，
`-netdev user`（slirp）自带 DHCP 服务器与虚拟网关，**不需要主机侧跑任何服务**：

- `vendor=0x1af4 device_id=0x1000`：legacy 网卡被 PCI 枚举到（modern 0x1041 明确拒绝）；
- `virtio-net: mac=52:54:00:12:34:56`：MAC **从设备配置空间读到**（驱动协商了 `VIRTIO_NET_F_MAC`；
  兜底地址是 `02:00:00:00:00:01`，因此看到 QEMU 的默认值就只可能是「真的读了配置空间」）；
- `net: arp 10.0.2.2 -> 52:55:0a:00:02:02 (reply)`：ARP 广播请求 → 单播应答，MAC 正是 slirp 网关；
- `net: dhcp offer yiaddr=10.0.2.15 server=10.0.2.2 mask=255.255.255.0 router=10.0.2.2`：
  UDP 68→67 的 DISCOVER → OFFER 往返，报文里的地址 / 掩码 / 网关都被解析出来
  —— 这是 Ethernet → IPv4 → UDP 收发两条路径都通了的证据；
- `net: icmp echo id=0x4242 seq=7 payload=13 reply from 10.0.2.2` + `net: icmp PASS`：
  ICMP Echo 请求（IPv4 `proto=1`）→ 回显**逐字节相同**载荷的应答 —— ICMP 路径也通了。

**边界（与 `docs/architecture.md` 第 5 节一致，别读成已完成）**：ICMP 只做 **Echo（ping）**，
不做差错报文；不做 TCP（`network/tcp.rs` 仍是占位）、不做重传 / 窗口；不做 IP 分片重组（收到 `MF` 置位或 offset ≠ 0 的 IPv4 报文直接丢弃）；
`network/socket.rs`（面向用户态的 UDP socket）**已在 v1.0 第四步落地**（见 1.9）；TCP 仍为占位。中断仍不配置 —— 与块设备一样走同步轮询。

### 1.9 v1.0 第四步已落地：UDP 套接字最小集（`socket` / `bind` / `sendto` / `recvfrom`）

把 1.8 的内核内部网络闭环接到用户态：`AF_INET` + `SOCK_DGRAM` 的 BSD 风格最小集，
号沿用 Linux（`socket=41`、`sendto=44`、`recvfrom=45`、`bind=49`），另加 Cerlesse 扩展号
`net_config=267` 导出 DHCP 协商到的本机地址/网关/MAC。

| 文件 | 职责 |
| ---- | ---- |
| `kernel/src/fs/file.rs` | fd 槽位泛化为 `Slot::{File, Socket(id)}`：套接字与文件**共用一个 fd 编号空间**；`close` 返回 `ClosedFd` 以回收套接字 |
| `kernel/src/network/socket.rs` | 套接字表（8 槽）+ 临时端口分配（49152 起）+ `bind` 冲突检测 |
| `kernel/src/network/mod.rs` | `NetConfig`（DHCP 成功后 `set_config`）、`udp_send`（目的 MAC 按广播/网关/ARP 选择）、`udp_recv_poll`（非阻塞轮询一次） |
| `kernel/src/syscall/socket.rs` | 四个处理器 + `sys_net_config`，按页校验用户缓冲 |
| `userland/libc/src/socket.rs` | `SockAddrIn` / `NetConfig` 的用户态形状（端口/地址网络字节序） |

**验收（`make test-network`，30 项）**：`/init` 用 socket API 向 slirp 的内置 DHCP 服务器
发 DISCOVER、收 OFFER 并核对 —— 一次**真实的、自包含的** UDP 往返：

- `user: socket domain=AF_INET type=SOCK_DGRAM fd=3 id=0` / `hello: bind ret=0`；
- `user: sendto fd=3 sport=68 dst=255.255.255.255:67 len=300 frame=342`；
- `user: recvfrom fd=3 src=10.0.2.2:67` + `hello: socket dhcp op=2 cookie=1 yiaddr=10.0.2.15`；
- `hello: socket close ret=0` / `hello: socket PASS=1`。

负例：`hello: socket bad-family ret=-97` / `socket stream ret=-93` /
`bind on file fd ret=-88` / `recvfrom unbound ret=-22` / `bind in-use ret=-98`。
没有网卡时 `net_config` 返回 `-ENODEV`，`/init` 打印 `socket skip network (no network device)`
并以 `socket PASS=1` 结束（`make test-user` 也照跑那几条不需要网卡的负例）。

**边界**：TCP、`connect`/`listen`/`accept`、`MSG_*`、等待队列、`SO_*` 选项均未做，
明确拒绝而不是静默忽略（详见 `docs/syscall.md` 3.14）。

## 2. 开发路线

### 2.1 v0.8（Driver Manager → PCI → 块设备 → 磁盘 FS 挂载）

1. [x] 驱动框架：`Driver` trait + 注册表 + 匹配/挂接统计（`driver/driver.rs`）。
2. [x] PCI 枚举：遍历总线、解析类别/BAR、按 Vendor/Device ID 匹配。
3. [x] 块设备抽象（`block.rs`）与 virtio-blk（legacy）驱动（`virtio_blk.rs`）。
4. [x] **磁盘文件系统后端：FAT12/16/32 只读挂载进 VFS**（`fs/fat.rs` + `fs::mount_disks`，见 `docs/filesystem.md`）。
5. [x] 验收补齐：`make test-fat` 从磁盘读到文件（内核自己的 `/disk/…KERNEL.ELF` 与测试盘上的各文件）。
     注：Shell 里的 `ls`/`cat` 属 v0.9；VFS 层的磁盘读取已在 v0.8 验收。

验收（v0.8）：

- [x] PCI 列表输出，包含 Vendor/Device ID（`make test-driver`）
- [x] 磁盘读写可验证（引导扇区读 + 最后扇区写回读 + 分块读一致性 + 负例）
- [x] FAT 挂载完成，可在 VFS 层访问磁盘文件（`make test-fat` 21 项：读内核映像 / 跨簇逐字节公式 / LFN / 空文件 / 偏移读 / `EROFS` 负例）

### 2.2 v0.9（网络栈顺序 + Socket syscall 最小集）

1. [x] Network Driver：`virtio-net`（legacy，复用 v0.8 的 `Driver` 框架与 PCI 枚举）—— `driver/virtio_net.rs`
2. [x] Ethernet → ARP → IPv4 → UDP（`network/{ethernet,arp,ipv4,udp,dhcp}.rs`，验收见 1.8）
3. [x] **ICMP（Echo/ping）**（`network/icmp.rs` + `Stack::icmp_ping`，验收见 1.8）
4. [x] **Socket API（UDP 最小集）**：`socket`/`bind`/`sendto`/`recvfrom` + `net_config`（v1.0 第四步，见 1.9）
5. [ ] TCP（尽力） → 面向流的 Socket（仍为占位）

验收（v0.9）：

- [x] Shell 交互可用（`make test-shell`，v0.9 第二步）
- [x] 网络最小闭环：ARP 解析网关 + DHCP 的 UDP 往返（`make test-network`）
- [x] **ping（ICMP Echo）**：对网关发 Echo 并校验回显载荷（`make test-network`，见 1.8）
- [ ] HTTP 简例（TCP 落地后再做）

## 3. 驱动列表

### 3.1 已实现

| 驱动 | 版本 | 说明 |
| ---- | ---- | ---- |
| Serial（串口） | v0.1 | 开发期唯一日志通道（`print`/`println`/`print_dec`/`print_hex`/`print_hex_pad`/`put_bytes`） |
| Keyboard（PS/2） | v0.4 | IRQ1 扫描码解码与串口回显 |
| Timer（PIT） | v0.4 | 100Hz tick：调度时间片与块 I/O 超时基准 |
| PCI | v0.8 | 枚举与 BAR 探测（配置机制 #1） |
| Disk（virtio-blk legacy） | v0.8 | 块设备：`BlockDevice` trait 的实现者 |
| Net（virtio-net legacy） | v0.9 | 网卡：`NetworkDevice` trait 的实现者（RX/TX 双队列） |
| Disk FS（FAT12/16/32） | v0.8 | 不是 PCI 驱动，而是 `FileSystem` 后端（`fs/fat.rs`）：消费 `BlockDevice` 提供的扇区 |

### 3.2 后续（按需补）

- AHCI/NVMe、USB、Audio、GPU、Framebuffer（视需要）。

## 4. 非目标 / 后续

- SMP 多核下的中断均衡与高级 APIC 语义：v1.0 之后。
- USB、Wi-Fi、GPU、完整 TCP/IP 工业栈：非第一版目标。
- DMA 缓冲管理细节：v0.8 只做了「连续物理帧」这一最小的 DMA 前置条件；IOMMU/散列（scatter-gather）
  留给真实硬件需求出现时。

## 5. 修订记录

| 日期 | 变更 |
| ---- | ---- |
| 2026-10-06 | 第一阶段创建占位 |
| 2026-10-07 | 补记当前状态：PIC 第一版已接入 kernel_main；v0.4 剩余（Timer/键盘/切换原语）与 v0.8/v0.9 待编写 |
| 2026-10-08 | v0.4 完成：IRQ stub 修正（B-08）、IRQ 入口保存寄存器（B-09）、PIT 100Hz、PS/2 键盘回显、上下文切换原语；新增 `make test-keyboard` |
| 2026-10-08 | v0.4 最后一项 H-04/H-05 落地：Go 编排器 + 串口分析器；`make go-test` / `make test-orch` 通过 |
| 2026-10-08 | 统一文档顶部元信息与修订记录（阶段 / 最后更新 / 状态） |
| 2026-10-11 | **v0.9 输入侧**：键盘驱动加行缓冲（行编辑 + 整行交付 + `kbd: line ready len=N`），回显由 `KB <字符>` 改为终端式原样回显；1.6 补记、1.7 限制项由「无环形缓冲」改为「单行缓冲 + cooked 回显」；`make test-keyboard` 断言同步 |
| 2026-10-11 | **v0.8 收官**：磁盘文件系统 FAT12/16/32（`fs/fat.rs`，只读）挂到 VFS，`fs::mount_disks` 接入启动链（`/disk` + `/mnt/fat`）；H-03 `tools/mkfs/mkfs.py` 落地（mtools 独立复核）；新增 `make test-fat`（21 项，双盘）与编排场景 `fat`（10 场景）；状态行改为「v0.8 已完成」，限制项从「FAT32 未做」改为「FAT 只读、无扇区缓存」 |
| 2026-10-11 | **v0.8 前半落地**：新增 `driver/pci.rs`（机制 #1 + BAR 探测 + 窗口大小）、`driver/driver.rs`（注册框架 + `probe_all`）、`driver/block.rs`（按扇区读写 + 验收自测）、`driver/virtio_blk.rs`（legacy virtio 0.9.5，同步轮询 + bounce buffer）、`arch/x86_64/port.rs`（端口 I/O）、`frame::allocate_contiguous`；启动盘改为 virtio-blk（Makefile / `tests/test_boot.py` / `tests/test_keyboard.py`，`disable-modern=on`）；新增 `make test-driver`（22 项）与编排场景 `driver`（9 场景）；修 B-16（BAR 大小 32→64 位宽错误）、B-17（LBA 溢出未拦）；状态行由「v0.8 尚未编写」改为「v0.8 前半完成，FAT32 待做」 |
| 2026-10-13 | **v1.0 第四步：UDP 套接字最小集** —— 新增 1.9；`fs/file.rs` 的 fd 槽位泛化为 `Slot::{File,Socket}`；新增 `network/socket.rs`（8 槽表 + 端口分配）与 `network/mod.rs` 的 `NetConfig`/`udp_send`/`udp_recv_poll`；新增 `syscall/socket.rs`（`socket`/`bind`/`sendto`/`recvfrom` + `net_config`）；`libc` 新增 `socket` 模块与包装；`user/hello` 新增 `socket_selftest`（DHCP DISCOVER→OFFER + 5 负例）；`make test-network` 15 → **30 项**；TCP 仍为占位 |
| 2026-10-11 | **v0.9 简化网络栈落地**：新增 `driver/net.rs`（`NetworkDevice` trait + 注册表）、`driver/virtio_net.rs`（legacy virtio-net 0.9.5：RX/TX 双队列、10 字节 virtio-net 头、MAC 读自设备配置空间、同步轮询）、`network/{ethernet,arp,ipv4,udp,dhcp}.rs` 与 `network/mod.rs`（`Stack` + `selftest`）；`network/{tcp,socket}.rs` 占位说明更新；`driver/driver.rs::register_builtin` 追加 virtio-net；`main.rs` 在磁盘验收后跑网络自测（无网卡时跳过）；新增 `make test-network`（13 项，QEMU 加 legacy virtio-net-pci + `-netdev user`）与编排场景 `network`（12 → **13 场景**）；`tests/test_boot.py` 增 `--nic`；新增 1.8 节与状态行、驱动列表更新 |
| 2026-10-12 | **ICMP（Echo/ping）落地** —— 新增 `network/icmp.rs`（Echo 组帧/解析，校验和复用 `ipv4::checksum`）与 `ipv4::PROTO_ICMP`；`network/mod.rs::Stack::icmp_ping`（Echo 请求 → 校验 Ethernet/IPv4/源地址/类型/id/seq 与**回显载荷逐字节相同**）；`selftest` 在 DHCP 之后向网关发 ping（`make test-network` 13 → **15 项**，`orchestrate.json` 的 `network` 场景同步）；状态行 / 1.8 / 2.2 清单（ICMP 勾选）/ 边界同步 |
