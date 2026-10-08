# Cerlesse OS — 构建入口（v0.1 实现）
# 详见 DEVELOPMENT.md 第 5、7 节。

CARGO ?= cargo
QEMU  ?= qemu-system-x86_64
PYTHON ?= python3
GO    ?= go

BUILD := build
OVMF_CODE ?= /usr/share/OVMF/OVMF_CODE.fd
OVMF_VARS ?= /usr/share/OVMF/OVMF_VARS.fd

BOOT_EFI := target/x86_64-unknown-uefi/debug/boot.efi
KERNEL_ELF := target/x86_64-unknown-none/debug/kernel

.PHONY: help build boot kernel image image-exception image-pagefault run test test-exception test-memory test-pagefault test-keyboard test-scheduler test-all go-test test-orch clean

help:
	@echo "targets:"
	@echo "  build  - build bootloader + kernel + disk image"
	@echo "  run    - boot the image in QEMU (serial on stdio)"
	@echo "  test   - integration test: assert 'Kernel started!' on serial"
	@echo "  test-exception - v0.2: trigger #DE, assert exception diagnostics"
	@echo "  test-memory - v0.3/v0.4: assert frame/heap/paging/context-switch self-tests"
	@echo "  test-pagefault - v0.3: touch unmapped address, assert #PF diagnostics"
	@echo "  test-keyboard - v0.4: inject keys via QEMU monitor, assert IRQ1 echo + PIT heartbeat"
	@echo "  test-scheduler - v0.5: assert >=3 kernel tasks rotate under PIT time slices"
	@echo "  test-all - run every integration test"
	@echo "  go-test  - v0.4 H-04/H-05: Go unit tests for tools-go (host side)"
	@echo "  test-orch - v0.4 H-04: run all 6 scenarios in parallel via Go orchestrator"
	@echo "  clean  - remove build artifacts"

build: image

boot:
	$(CARGO) build -p boot --target x86_64-unknown-uefi

kernel:
	$(CARGO) build -p kernel --target x86_64-unknown-none

image: boot kernel
	$(PYTHON) tools/image-builder/build_image.py \
		--boot $(BOOT_EFI) \
		--kernel $(KERNEL_ELF) \
		--out $(BUILD)/disk.img

run: image
	cp $(OVMF_VARS) $(BUILD)/OVMF_VARS.fd
	$(QEMU) -machine q35 -m 512M \
		-drive if=pflash,format=raw,readonly=on,file=$(OVMF_CODE) \
		-drive if=pflash,format=raw,file=$(BUILD)/OVMF_VARS.fd \
		-drive format=raw,file=$(BUILD)/disk.img \
		-serial stdio -display none -no-reboot

test: image
	$(PYTHON) tests/test_boot.py

image-exception: boot
	$(CARGO) build -p kernel --target x86_64-unknown-none --features exception-test
	$(PYTHON) tools/image-builder/build_image.py \
		--boot $(BOOT_EFI) \
		--kernel $(KERNEL_ELF) \
		--out $(BUILD)/disk-exception.img

image-pagefault: boot
	$(CARGO) build -p kernel --target x86_64-unknown-none --features pagefault-test
	$(PYTHON) tools/image-builder/build_image.py \
		--boot $(BOOT_EFI) \
		--kernel $(KERNEL_ELF) \
		--out $(BUILD)/disk-pagefault.img

test-exception: image-exception
	$(PYTHON) tests/test_boot.py \
		--disk $(BUILD)/disk-exception.img \
		--serial-log $(BUILD)/serial-exception.log \
		--expect "Kernel started!" \
		--expect "EXCEPTION: divide error"

# v0.3/v0.4 内核自测验收：帧分配器 + 内核堆 + 页表 Mapper + 上下文切换原语
test-memory: image
	$(PYTHON) tests/test_boot.py \
		--serial-log $(BUILD)/serial-memory.log \
		--expect "Kernel started!" \
		--expect "heap: Box/Vec/String PASS" \
		--expect "frame: stress PASS" \
		--expect "paging: map/unmap PASS" \
		--expect "context: switch PASS"

# v0.3 验收：页错误诊断（feature 门控注入未映射地址访问）
test-pagefault: image-pagefault
	$(PYTHON) tests/test_boot.py \
		--disk $(BUILD)/disk-pagefault.img \
		--serial-log $(BUILD)/serial-pagefault.log \
		--expect "Kernel started!" \
		--expect "EXCEPTION: page fault" \
		--expect "CR2=0x500000000000" \
		--expect "KERNEL PANIC: unhandled exception"

test-all: test test-exception test-memory test-pagefault test-keyboard test-scheduler

# v0.5 验收：≥3 个内核任务在 PIT 时间片驱动下轮转（Round Robin）
test-scheduler: image
	$(PYTHON) tests/test_boot.py \
		--serial-log $(BUILD)/serial-sched.log \
		--expect "Kernel started!" \
		--expect "sched: spawn pid=1 name=task_a" \
		--expect "task_a: entered" \
		--expect "task_b: entered" \
		--expect "task_c: entered" \
		--expect "sched: switch pid=3 -> 0" \
		--expect "sched: round-robin wrap PASS"

# v0.4 验收：PIT 心跳递增 + PS/2 键盘 IRQ1 扫描码回显（经 QEMU monitor 注入真实按键）
test-keyboard: image
	$(PYTHON) tests/test_keyboard.py \
		--disk $(BUILD)/disk.img \
		--serial-log $(BUILD)/serial-kb.log \
		--key a \
		--key b \
		--expect "KBIRQ enabled" \
		--expect "KBIRQ ENTRY" \
		--expect "KB a" \
		--expect "KBIRQ EXIT" \
		--expect "IRQ0_heartbeat tick="

# v0.4 H-04/H-05 主机侧：Go 单元测试（串口分析 + 并行编排器）
go-test:
	cd tools-go && $(GO) vet ./... && $(GO) test ./...

# v0.4 H-04 验收：Go 编排器并行跑全部 6 个集成测试场景 + 串口日志二次复核。
# 磁盘镜像先构建（默认/异常注入/页错误三个镜像），随后场景并行执行；
# 每个场景独占 OVMF_VARS 副本、串口日志与 monitor socket（见 tests/orchestrate.json）。
test-orch: image image-exception image-pagefault
	cd tools-go && $(GO) run ./cmd/testorch -config ../tests/orchestrate.json -root ..

clean:
	rm -rf $(BUILD)
