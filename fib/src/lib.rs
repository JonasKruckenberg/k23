#![no_std]

#[no_mangle]
pub extern "C" fn fib(n: u32) -> u32 {
    let mut a = 1;
    let mut b: u32 = 1;
    for _ in 0..n {
        let t = a;
        a = b;
        b += t;
    }
    return b;
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
