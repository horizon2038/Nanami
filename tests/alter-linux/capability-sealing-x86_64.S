.text
.global seal_probe_capability
seal_probe_capability:
    xor %esi, %esi
    mov $-1, %rax  /* A9N CapabilityCall, not Linux syscall emulation. */
    syscall
    test %rdi, %rdi
    jne 2f
    cmp $2, %rsi  /* InvalidDescriptor. */
    jne 2f
    xor %eax, %eax
    ret
2:
    mov $1, %eax
    ret
.global seal_clone_vm
seal_clone_vm:
    mov %rdi, %rsi
    mov $0x111, %edi  /* CLONE_VM | SIGCHLD, independent task and capability root. */
    xor %edx, %edx
    xor %r10d, %r10d
    xor %r8d, %r8d
    mov $56, %eax
    syscall
    test %rax, %rax
    jne 1f
    call seal_shared_child
    mov %eax, %edi
    mov $60, %eax
    syscall
    ud2
1:
    ret
.section .note.GNU-stack,"",@progbits
