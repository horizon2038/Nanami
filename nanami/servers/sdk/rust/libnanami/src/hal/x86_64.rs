#[macro_export]
macro_rules! define_x86_64_entry {
    ($entry:path) => {
        use core::arch::global_asm;

        global_asm!(
            r#"
            .section .text
            .global _start
        _start:
            mov rbp, rsp
            call __nanami_app_entry
        1:
            pause
            jmp 1b
        "#
        );

        #[no_mangle]
        extern "C" fn __nanami_app_entry() -> ! {
            let result: $crate::NanamiResult = $entry();
            $crate::nanami_exit(result)
        }
    };
}
