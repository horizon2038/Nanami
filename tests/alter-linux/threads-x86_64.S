.text
.global thread_start
thread_start:
    sub $16, %rsi
    mov %r9, (%rsi)
    mov %rcx, %r10
    mov $56, %eax
    syscall
    test %rax, %rax
    jnz 1f
    mov (%rsp), %rax
    call *%rax
    mov %eax, %edi
    mov $60, %eax
    syscall
    ud2
1:
    ret
.global read_tls_word
read_tls_word:
    mov %fs:0, %rax
    ret
.section .note.GNU-stack,"",@progbits

/* Copied into a tiny standalone ELF by the file-mapping fixture. All internal
 * references are relative so its behavior does not depend on the source VA. */
.section .rodata.exec_stack_probe,"a",@progbits
.global exec_stack_probe_start
.global exec_stack_probe_end
exec_stack_probe_start:
    cmpb $0, 0x400f00
    jne .Lexec_stack_fail
    mov $0x4000000, %r12d
    cmpq $0, (%r12)
    jne .Lexec_stack_fail
    movb $0x51, (%r12)
    mov $2, %eax
    lea .Lexec_zero_path(%rip), %rdi
    xor %esi, %esi
    xor %edx, %edx
    syscall
    test %rax, %rax
    js .Lexec_stack_fail
    mov %rax, %rdi
    xor %eax, %eax
    mov $0x4001000, %esi
    mov $4096, %edx
    syscall
    cmp $4096, %rax
    jne .Lexec_stack_fail
    cmpq $0, 0x1000(%r12)
    jne .Lexec_stack_fail
    mov $3, %eax
    syscall
    mov $57, %eax
    syscall
    test %rax, %rax
    js .Lexec_stack_fail
    jz .Lexec_stack_child
    mov %rax, %rdi
    movl $-1, (%rsp)
    mov %rsp, %rsi
    xor %edx, %edx
    xor %r10d, %r10d
    mov $61, %eax
    syscall
    test %rax, %rax
    js .Lexec_stack_fail
    cmpl $0, (%rsp)
    jne .Lexec_stack_fail
    cmpb $0x51, (%r12)
    jne .Lexec_stack_fail
    cmpb $0, 0x2000(%r12)
    jne .Lexec_stack_fail
    jmp .Lexec_stack_ok
.Lexec_stack_child:
    cmpb $0x51, (%r12)
    jne .Lexec_stack_fail
    cmpb $0, 0x2000(%r12)
    jne .Lexec_stack_fail
    movb $0x72, (%r12)
    movb $0x73, 0x2000(%r12)
.Lexec_stack_ok:
    mov $10, %eax
    mov $0x400000, %edi
    mov $4096, %esi
    mov $7, %edx
    syscall
    test %rax, %rax
    jne .Lexec_stack_fail
    movb $0x8c, 0x400f00
    cmpb $0x8c, 0x400f00
    jne .Lexec_stack_fail
    xor %edi, %edi
    jmp .Lexec_stack_exit
.Lexec_stack_fail:
    mov $227, %edi
.Lexec_stack_exit:
    mov $60, %eax
    syscall
    ud2
.Lexec_zero_path:
    .asciz "/dev/zero"
exec_stack_probe_end:
