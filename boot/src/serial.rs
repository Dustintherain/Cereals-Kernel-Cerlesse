//! Bootloader 串口输出：16550 UART @ COM1（与内核 serial 相同的初始化序列）

const COM1: u16 = 0x3F8;

#[inline]
unsafe fn outb(port: u16, val: u8) {
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") port,
            in("al") val,
            options(nomem, nostack, preserves_flags)
        );
    }
}

#[inline]
unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    unsafe {
        core::arch::asm!(
            "in al, dx",
            in("dx") port,
            out("al") val,
            options(nomem, nostack, preserves_flags)
        );
    }
    val
}

pub fn init() {
    unsafe {
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x80);
        outb(COM1 + 0, 0x03);
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x03);
        outb(COM1 + 2, 0xC7);
        outb(COM1 + 4, 0x0B);
    }
}

fn putc(c: u8) {
    unsafe {
        while inb(COM1 + 5) & 0x20 == 0 {}
        outb(COM1, c);
    }
}

pub fn print(s: &str) {
    for b in s.bytes() {
        putc(b);
    }
}

pub fn println(s: &str) {
    print(s);
    putc(b'\r');
    putc(b'\n');
}

pub fn print_hex(val: u64) {
    print("0x");
    let mut started = false;
    for i in (0..16).rev() {
        let nib = ((val >> (i * 4)) & 0xF) as u8;
        if nib != 0 {
            started = true;
        }
        if started {
            putc(if nib < 10 { b'0' + nib } else { b'a' + nib - 10 });
        }
    }
    if !started {
        putc(b'0');
    }
}
