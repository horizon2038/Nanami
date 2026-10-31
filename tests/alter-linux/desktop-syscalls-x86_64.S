.text
.global clone3_child
clone3_child:
    push %rbx
    mov %rdx, %rbx
    mov $435, %eax
    syscall
    test %rax, %rax
    jnz 1f
    call *%rbx
    xor %edi, %edi
    mov $60, %eax
    syscall
    ud2
1:
    pop %rbx
    ret
.section .note.GNU-stack,"",@progbits
