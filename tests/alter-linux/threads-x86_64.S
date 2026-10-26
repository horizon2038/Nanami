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
