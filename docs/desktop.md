# 桌面环境兼容路线（v1.0 之后的规划）

- 阶段：桌面兼容（规划，**未开工**）
- 最后更新：2026-10-11
- 状态：📋 规划。本文档回答「要让 X11 / Wayland 合成器、PipeWire/PulseAudio、
  systemd-logind/udev、D-Bus 这类**用户态桌面栈**在 Cerlesse 上有地基可跑，内核缺什么、
  按什么顺序补、每一步怎么在 QEMU 里断言」；不改变 v1.0 的范围（见 roadmap.md / DEVELOPMENT.md）。
- 关联文档：[roadmap.md](roadmap.md) · [architecture.md](architecture.md)（第 5 节非目标）·
  [drivers.md](drivers.md) · [syscall.md](syscall.md) · [filesystem.md](filesystem.md)

> 本文档顶部保留 阶段 / 最后更新 / 状态 字段，与项目文档维护约定对齐。

## 0. 边界（先读这一段）

- **桌面栈是海量用户态代码**（glibc/systemd/mesa/wayland/dbus/pipewire…）。本文档只规划**内核侧**；
  「随意安装各种桌面环境」是多年目标，这里定义的是**可验证的渐进里程碑**，不是一次交付。
- 所有条目**均未实现**（除「已有」列出的现状）；每个里程碑必须落成 `make test-*` 的串口/监控台断言。
- 明确的「不做」：GPU 3D 加速、完整 ACPI（AML 解释器）、完整 DRM（GPU 调度/GEM 共享/显存管理）、
  USB / Wi-Fi、完整 POSIX —— 与 architecture.md 第 5 节保持一致。
- 「接口对齐」的含义：**ioctl/syscall/设备节点形状尽量沿用 Linux 语义**（本项目既有惯例：
  0..=255 号段 = Linux 语义，>= 256 = Cerlesse 扩展），这样真实用户态才有机会直接跑。

## 1. 先纠正四类归属（用户清单里混了内核与用户态）

| 清单里的项 | 实质 | 内核侧真正要提供的地基 |
| ---- | ---- | ---- |
| systemd-logind | **用户态守护进程**（D-Bus 服务）：会话/座位/电源请求 | 设备模型（`/dev` + `uevent` → udev 能枚举设备）、cgroup v2（会话作用域）、`reboot`/挂起的内核入口、AF_UNIX + `SO_PEERCRED`（D-Bus 鉴权） |
| D-Bus | **用户态消息总线** | AF_UNIX（含 fd 传递 `SCM_RIGHTS` 与 `SO_PEERCRED`）、`poll/epoll`、信号 |
| PipeWire / PulseAudio | **用户态声音服务器** | ALSA 驱动框架 + 具体驱动 + `/dev/snd` 节点 + ioctl + mmap（DMA 环） |
| X11 / Wayland 合成器 | **用户态显示服务器** | fbdev（X11 起步）/ DRM+KMS+GEM dumb（Wayland）、设备内存 `mmap`、`evdev` 输入、`memfd`/`eventfd`/`timerfd` |
| KVM | **不是桌面前置**：那是「Cerlesse 当宿主跑 VM」的虚拟化方向 | 开发效率靠**宿主**的 QEMU/KVM；与桌面兼容正交，单独成方向、不参与本路线优先级 |

## 2. 现状审计（2026-10-11，逐项有据）

| 能力 | 现状 | 证据/位置 |
| ---- | ---- | ---- |
| 显示 | **无**。`driver/framebuffer.rs` 是**未注册**的占位（`TODO(v0.9): 简单文本输出；GUI 非第一版目标`） | `kernel/src/driver/framebuffer.rs`；`driver/mod.rs` 的模块表里没有它 |
| DRM/KMS 对象（CRTC/Plane/Encoder/Connector/Framebuffer） | **无** | — |
| 输入核心 / evdev | **无**。PS/2 键盘只有一条私有路径：IRQ1 → set 1 解码 → 128 字节行缓冲 → 串口回显 | `driver/keyboard.rs`（`decode_set_1_scan`/`take_line`，不是 evdev 事件） |
| uinput | **无** | — |
| ioctl | **无**（syscall 表：`0..=255` Linux 号段里已实现 read/write/open/close/mmap/brk/getpid/sleep/fork/exec/exit/wait/getdents；扩展号段 `getprocs=256`、CSS `storage_volumes=261`/`storage_rescan=265`；`SYS_MAX=267`） | `abi/src/syscall.rs`（单一事实源）+ `syscall/syscall_table.rs`（排表） |
| poll / epoll / select | **无** | — |
| 设备模型 / `/dev` | **无**：没有设备节点、没有设备号；fd 0/1/2 是保留位（stdin=键盘行缓冲、stdout/stderr=串口），fd ≥ 3 一律是 RAMFS 文件 | `fs/file.rs`、`docs/syscall.md` 2.7 |
| `/proc` / `/sys` | **无**（进程表靠扩展 syscall `getprocs=256` 暴露；udev/logind/`ps`/`top` 都读 `/proc`、`/sys`） | `syscall/process.rs::sys_getprocs` |
| file-backed `mmap` | **无**：`mmap` 只收 `MAP_PRIVATE|MAP_ANONYMOUS`（文件映射明确返回 `EINVAL`） | `syscall/memory.rs` |
| 信号 | **无** | — |
| futex / 线程 | **无**（用户态单线程；调度器只有内核任务） | `docs/process.md` |
| AF_UNIX / socket | **无**（网络只有 Ethernet → ARP → IPv4 → UDP + DHCP；`tcp.rs`/`socket.rs` 仍为占位） | `kernel/src/network/{tcp,socket}.rs` |
| 音频 | **无** | — |
| ACPI | **无**：PCI 用配置机制 #1，刻意避开 ACPI（ECAM 需要 MCFG）；无关机/热区/电池 | `driver/pci.rs` 注释、DEVELOPMENT.md 设计取舍 |
| 会话/权限 | `security/` 三个占位文件；无 uid/gid、无设备 ACL、无 `setuid` | `kernel/src/security/{capability,credential,permission}.rs` |
| 文件系统写 | **只读**（FAT12/16/32 只读；RAMFS 非持久） | `fs/fat.rs`、`docs/filesystem.md` |
| fs 通知（inotify） | **无** | — |
| 测试抓手 | **已具备**：QEMU monitor 走 unix socket（`sendkey` 已在 `test_keyboard.py` 用），且本机 QEMU 6.2 实测 `screendump`（`-display none` 下产出 PPM）、`mouse_move`/`mouse_button` 可用 | `tests/test_keyboard.py`；`/tmp/s.ppm` 实测 921 KB |

## 3. 被清单漏掉的地基（不补这些，接口层做不出来）

1. **设备模型**：`/dev` 节点 + 设备号 + fd 到驱动的路由 + `uevent`（netlink 广播）—— udev/logind 的输入。
2. **`ioctl`**：DRM / evdev / ALSA / ACPI 的**全部**接口都是 ioctl；没有它，「接口对齐」无从谈起。
3. **`poll`/`epoll`**：每个合成器/事件循环/音频服务器的 `while(1) poll()` 主循环。
4. **file-backed `mmap`**：DRM dumb buffer、ALSA DMA 环、fbdev 显存映射都要它。
5. **futex + 信号 + 线程**：现代用户态的并发原语（glibc/mesa/pipewire 全依赖）。
6. **`/proc` 与 `/sys`**：桌面工具、udev、logind 的「事实来源」。
7. **AF_UNIX + `SO_PEERCRED` + `SCM_RIGHTS`**：D-Bus/logind 的传输层与鉴权。
8. **`memfd`/`eventfd`/`timerfd`**：Wayland 共享缓冲与事件循环。
9. **高质量时钟**（`clock_gettime` 单调/纳秒）：合成器帧调度与音频时间线。

## 4. 分层路线（每层 = 一个可在 QEMU 里断言的里程碑）

### D0 地基 —— ioctl / poll / 设备模型 / `/proc`+`/sys`

- [ ] 设备模型：`mkdev` 风格的设备注册 + `/dev` 节点（`open` 路由到驱动）+ 简化 `uevent` 通道
- [ ] `ioctl`（`fd + cmd + arg`，按 fd 类型分发；fd 表从「RAMFS 文件」扩成 `File`/`Device` 枚举）
- [ ] `poll`（可读/可写/超时；先覆盖 evdev、pipe、timer、设备）
- [ ] file-backed `mmap`（先设备内存，后普通文件）
- [ ] futex 最小集（wait/wake）+ 信号最小集（SIGTERM/SIGINT/SIGCHLD）
- [ ] `/proc` 最小只读视图（`/proc/<pid>/stat`、`/proc/self/*`）+ `/sys` 只读设备树
- 验收：用户程序走通 `open("/dev/x")` → `ioctl` → `poll` 三段并留下串口断言；`/proc` 视图与
  `getprocs` 的记录一致；uevent 消息内容可在测试侧断言

### D1 输入子系统（evdev + uinput）

- [ ] 输入核心（`driver/input.rs`）：`input_event {time, type, code, value}` + 每设备事件队列 + `EV_SYN` 分隔
- [ ] PS/2 键盘接到输入核心（`EV_KEY`/`EV_MSC`；现有行缓冲改为 evdev 事件的消费者）
- [ ] PS/2 鼠标（i8042 aux 端口，QEMU `-device` 默认提供）→ `EV_REL`/`EV_KEY(BTN_LEFT)`
- [ ] `evdev` 设备 `/dev/input/eventN`：ioctl 子集（`EVIOCGVERSION/EVIOCGID/EVIOCGNAME/EVIOCGBIT/EVIOCGABS`）、
      `read` 返回 24 字节 `input_event`、`poll` 可读
- [ ] `uinput`（`/dev/uinput` + `UI_DEV_CREATE`/`UI_DEV_SETUP`/`UI_EVENT`）：用户态创建虚拟设备并注入事件
- [ ] （可选第二驱动）`virtio-keyboard-pci` / `virtio-tablet-pci`（QEMU 6.2 均可用）
- 验收：`make test-input` —— QEMU monitor `sendkey`/`mouse_move` 注入真实事件，用户程序读 evdev 把事件打成
  断言行；再用 uinput 注入一串事件断言回环（`libinput`/X11/Wayland 消费的就是这个接口）

### D2 显示与 DRM/KMS（最大的一块，分两跳）

**D2a fbdev —— 先让画面出现（不需要模式设置的复杂接口）**

- [ ] 启动链：OVMF 的 **GOP 线性帧缓冲**经 `BootInfo` 交给内核（现在 `BootInfo` 只有 memory_map，
      需要新增 base/宽/高/stride/格式字段 → 改 `shared/` 与 `boot/`）
- [ ] `/dev/fb0` + `FBIOGET_VSCREENINFO`/`FBIOGET_FSCREENINFO`（+ 简化 `FBIOPUT_VSCREENINFO`）+ 显存 `mmap`
- 验收：内核/用户程序画测试图案 → 测试侧用 QEMU monitor `screendump` 存 PPM → **逐像素断言**
  （QEMU 6.2 与现有 monitor socket 基建已实测具备）

**D2b DRM/KMS —— Wayland 的前提**

- [ ] KMS 对象模型：Framebuffer / CRTC / Encoder / Connector / Plane（第一版：1 CRTC + 1 连接器 + 1 主平面）
- [ ] DRM uapi 子集：`DRM_IOCTL_VERSION/GET_CAP/GET_RESOURCES/GET_CONNECTOR/MODE_CREATE_DUMB/MODE_MAP_DUMB/
      MODE_ADDFB2`（`MODE_ATOMIC` 可以后置，先 `SETCRTC` + page flip 事件）
- [ ] 驱动：先 `virtio-gpu-pci`（QEMU 首选，2D 命令 + 显示管线；本机 6.2 可用），`bochs-display` 备选
- 验收：自写的用户态测试走 `GET_RESOURCES → dumb buffer → 翻页`，`screendump` 逐像素校验；
  之后再用真实客户端（`modetest`/`kmscube`）做「接口对齐」的进阶验收（属后续，不算本里程碑）

### D3 音频（ALSA 用户接口子集 + 驱动）

- [ ] ALSA 终端：`/dev/snd/{controlC0,pcmC0D0p}` + ioctl（`HW_PARAMS/HW_FREE/PREPARE/WRITEI/START/STOP`）
- [ ] 驱动：QEMU 上先 **AC'97**（最简单，本机 6.2 可用）或 `intel-hda`（更标准、更复杂）；
      音频缓冲走 file-backed `mmap` 或 ioctl 拷贝
- [ ] 验收：用户程序写一段正弦波 → 断言 `prepared/started/underrun` 状态机；QEMU 侧用无声卡模式跑通接口。
  **明确**：接口验收 ≠ 听感验收（音频内容无法自动断言）

### D4 会话与电源（logind 的内核地基）

- [ ] ACPI 子集：解析 FADT/MADT（MADT 顺带为 APIC 铺路，R-04 的遗留项）；写 PM1a `SLP_TYP|SLP_EN` → S5
      （QEMU 会真的退出进程，可断言）；`reboot` 系统调用
- [ ] cgroup v2 最小集（会话/作用域树；先只读视图 + 设备访问控制的挂点）
- [ ] 设备 ACL 所需的 uid/gid（与 D5 联动）
- [ ] 热区/电池：QEMU 上可先用固定桩验证接口形状（真实平台支持放到后续）
- 验收：`make test-power` 断言 S5 后 QEMU 进程退出；`/sys` 能读到设备树快照

### D5 完整性收尾

- [ ] 网络：TCP + `AF_INET` socket + 接入 `poll`；DNS 由用户态自理（内核只欠 TCP/socket）
- [ ] 文件系统：FAT 写路径（簇分配 + 写回）或引入 ext2 读写；`inotify`；写回策略
- [ ] 权限：uid/gid、`chmod`/`chown`、设备节点 ACL、`setuid`（与 logind 的设备管理结合）
- [ ] `/proc`、`/sys` 扩大；`getrandom`
- 验收：文件管理器式操作（write/rename/inotify 通知）在 QEMU 里闭环

## 5. 优先级与取舍（与本项目路线的差异，含理由）

1. **同意**：显示与输入先于文件系统/网络的「大而全」（网络/文件系统的地基已在 v0.7–v0.9 打了大半）。
2. **修正**：接口层之前必须先做 **D0 地基**（ioctl/poll/设备模型/`/proc`+`/sys`）—— 否则做出的接口
   无法与真实用户态对齐；D0 也是后面所有层共享的前提。
3. **修正**：**D1 输入比 D2 显示更小、更独立、依赖更少**，建议先 D1 再 D2（清单把 DRM/KMS 排第一，
   但它是全清单里最重的；fbdev 一跳（D2a）足以先让 X11/控制台有画面）。
4. **修正**：systemd-logind / D-Bus / PipeWire **不在内核里** —— 它们的地基是 D0（uevent/`/dev`/AF_UNIX/
   `SO_PEERCRED`）、D1（evdev）与 D4（cgroup/电源）。
5. **修正**：**KVM 与桌面前置无关**（那是「Cerlesse 当宿主」的虚拟化方向）；开发效率用宿主
   QEMU/KVM（本项目测试就是这么跑的）。列入独立方向、不占桌面优先级。

## 6. 里程碑建议（v1.0 之后）

| 版本 | 目标 | 验收抓手 |
| ---- | ---- | ---- |
| **v1.1** | D0 地基：设备模型 + `ioctl` + `poll` + `/proc`/`/sys` 最小视图 | `make test-devfs` / `make test-procfs` |
| **v1.2** | D1 输入：输入核心 + evdev + uinput | `make test-input`（sendkey/mouse_move + uinput 回环） |
| **v1.3** | D2a fbdev + `screendump` 像素断言 | `make test-fb` |
| **v1.4** | D2b KMS 子集 + `virtio-gpu` 驱动 | `make test-kms` |
| **v1.5** | D3 音频 + D4 电源/会话地基 | `make test-snd` / `make test-power` |
| **v1.6+** | D5 网络/文件系统/权限收尾 | 逐项 |

依赖关系：`D0 → D1 → D2a → D2b`；`D3` 依赖 D0（ioctl/mmap）；`D4` 与 D0/D1 并行；
`D5` 可在任意空档穿插（不影响显示/输入主线）。

## 7. 修订记录

| 日期 | 变更 |
| ---- | ---- |
| 2026-10-11 | 创建本文档：把「桌面环境兼容」从零散讨论固化为内核侧路线 —— 现状审计（含证据）、
用户态/内核侧归属纠正、被漏掉的地基（ioctl/poll/设备模型/`/proc`/futex/AF_UNIX…）、
D0–D5 分层里程碑与 QEMU 可断言验收（`screendump`/`sendkey` 实测可用）、v1.1–v1.6 里程碑表 |
