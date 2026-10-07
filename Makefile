# Cerlesse OS — 构建入口（v0.1 实现）
# 详见 DEVELOPMENT.md 第 5、7 节。

CARGO ?= cargo
QEMU  ?= qemu-system-x86_64
PYTHON ?= python3

BUILD := build
OVMF_CODE ?= /usr/share/OVMF/OVMF_CODE.fd
OVMF_VARS ?= /usr/share/OVMF/OVMF_VARS.fd

BOOT_EFI := target/x86_64-unknown-uefi/debug/boot.efi
KERNEL_ELF := target/x86_64-unknown-none/debug/kernel

.PHONY: help build boot kernel image run test test-exception test-memory test-pagefault test-all clean

help:
	@echo "targets:"
	@echo "  build  - build bootloader + kernel + disk image"
	@echo "  run    - boot the image in QEMU (serial on stdio)"
	@echo "  test   - integration test: assert 'Kernel started!' on serial"
	@echo "  test-exception - v0.2: trigger #DE, assert exception diagnostics"
	@echo "  test-memory - v0.3: assert frame allocator / heap / paging self-tests"
	@echo "  test-pagefault - v0.3: touch unmapped address, assert #PF diagnostics"
	@echo "  test-all - run every integration test"
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

test-exception:
	$(CARGO) build -p kernel --target x86_64-unknown-none --features exception-test
	$(PYTHON) tools/image-builder/build_image.py \
		--boot $(BOOT_EFI) \
		--kernel $(KERNEL_ELF) \
		--out $(BUILD)/disk-exception.img
	$(PYTHON) tests/test_boot.py \
		--disk $(BUILD)/disk-exception.img \
		--serial-log $(BUILD)/serial-exception.log \
		--expect "Kernel started!" \
		--expect "EXCEPTION: divide error"

# v0.3 验收：帧分配器压力测试 + 内核堆（Box/Vec/String）+ 页表 Mapper 自测
test-memory: image
	$(PYTHON) tests/test_boot.py \
		--serial-log $(BUILD)/serial-memory.log \
		--expect "Kernel started!" \
		--expect "heap: Box/Vec/String PASS" \
		--expect "frame: stress PASS" \
		--expect "paging: map/unmap PASS"

# v0.3 验收：页错误诊断（feature 门控注入未映射地址访问）
test-pagefault:
	$(CARGO) build -p kernel --target x86_64-unknown-none --features pagefault-test
	$(PYTHON) tools/image-builder/build_image.py \
		--boot $(BOOT_EFI) \
		--kernel $(KERNEL_ELF) \
		--out $(BUILD)/disk-pagefault.img
	$(PYTHON) tests/test_boot.py \
		--disk $(BUILD)/disk-pagefault.img \
		--serial-log $(BUILD)/serial-pagefault.log \
		--expect "Kernel started!" \
		--expect "EXCEPTION: page fault" \
		--expect "CR2=0x500000000000" \
		--expect "KERNEL PANIC: unhandled exception"

test-all: test test-exception test-memory test-pagefault

clean:
	rm -rf $(BUILD)
