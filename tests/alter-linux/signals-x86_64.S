.text
.global _start
_start:
    call test_main
    mov %eax, %edi
    mov $60, %eax
    syscall
    ud2
.global linux_call
linux_call:
    mov %rdi, %rax
    mov %rsi, %rdi
    mov %rdx, %rsi
    mov %rcx, %rdx
    mov %r8, %r10
    mov %r9, %r8
    mov 8(%rsp), %r9
    syscall
    ret
.global signal_restorer
signal_restorer:
    mov $15, %rax
    syscall
    ud2
.global invalid_instruction
invalid_instruction:
    ud2
    ret
.section .note.GNU-stack,"",@progbits
