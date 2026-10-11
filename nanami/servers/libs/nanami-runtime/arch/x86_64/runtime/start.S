.global _start
.type _start, @function
_start:
    mov %rsp, %rbp
    call main
1:
    hlt
    jmp 1b
