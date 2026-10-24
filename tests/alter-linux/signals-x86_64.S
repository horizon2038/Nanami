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
.global spin_until_signal
spin_until_signal:
    push %rbx
    push %r12
    mov $0x11223344, %ecx
    mov $0x55667788, %r11d
    mov $0x99aabbcc, %ebx
    mov $0xddeeff00, %r12d
    mov $1000000000, %edx
1:
    cmp %esi, (%rdi)
    jne 2f
    dec %edx
    jne 1b
    mov $1, %eax
    jmp 4f
2:
    cmp $0x11223344, %ecx
    jne 3f
    cmp $0x55667788, %r11d
    jne 3f
    cmp $0x99aabbcc, %ebx
    jne 3f
    cmp $0xddeeff00, %r12d
    jne 3f
    xor %eax, %eax
    jmp 4f
3:
    mov $2, %eax
4:
    pop %r12
    pop %rbx
    ret
.section .note.GNU-stack,"",@progbits
