/* Freestanding x86-64 Linux fixture: no libc or host filesystem access.
 * clang --target=x86_64-linux-gnu -O2 -nostdlib -static -fno-builtin
 *       -fno-stack-protector -fuse-ld=lld -Wl,-e,_start -o writeback-test writeback.c
 */
typedef unsigned long usize;
static long syscall4(long n, long a, long b, long c, long d) {
    register long r10 __asm__("r10") = d;
    long result;
    __asm__ volatile("syscall" : "=a"(result) : "a"(n), "D"(a), "S"(b), "d"(c), "r"(r10)
                     : "rcx", "r11", "memory");
    return result;
}
#define sc(n,a,b,c) syscall4(n,(long)(a),(long)(b),(long)(c),0)
static unsigned char output[8192], input[8192];

long test_main(void) {
    if (sc(74,-1,0,0) != -9 || sc(75,-1,0,0) != -9 || sc(306,-1,0,0) != -9) return 1;
    long fd = sc(2,"/writeback-data",2|64|512,0644);
    if (fd < 0) return 2;
    for (usize i=0; i<sizeof(output); ++i) output[i] = 0x55;
    if (sc(1,fd,output,sizeof(output)) != sizeof(output)) return 3;
    if (syscall4(17,fd,(long)input,sizeof(input),0) != sizeof(input)) return 4;
    for (usize i=0; i<sizeof(input); ++i) if (input[i] != 0x55) return 5;
    long copy = sc(32,fd,0,0);
    if (copy < 0 || sc(3,fd,0,0) != 0 || sc(74,copy,0,0) != 0) return 6;
    fd = copy;
    if (sc(8,fd,0,0) != 0) return 7;
    // More than the cache budget: exercise pressure writeback and indirect blocks.
    for (usize block=0; block<256; ++block) {
        for (usize i=0; i<sizeof(output); ++i) output[i] = (unsigned char)(block+i*17);
        if (sc(1,fd,output,sizeof(output)) != sizeof(output)) return 8;
    }
    if (sc(75,fd,0,0) != 0 || sc(306,fd,0,0) != 0 || sc(162,0,0,0) != 0) return 9;
    for (usize block=0; block<256; ++block) {
        if (syscall4(17,fd,(long)input,sizeof(input),block*sizeof(input)) != sizeof(input)) return 10;
        for (usize i=0; i<sizeof(input); ++i)
            if (input[i] != (unsigned char)(block+i*17)) return 11;
    }
    if (sc(3,fd,0,0) != 0 || sc(74,fd,0,0) != -9) return 12;
    fd = sc(2,"/writeback-sync",2|64|512|04010000,0644);
    if (fd < 0) return 13;
    if ((sc(72,fd,3,0) & 04010000) != 04010000 || sc(72,fd,4,0) != 0
        || (sc(72,fd,3,0) & 04010000) != 04010000) return 15;
    for (usize i=0; i<sizeof(output); ++i) output[i] = 0x73;
    if (sc(1,fd,output,sizeof(output)) != sizeof(output) || sc(3,fd,0,0) != 0) return 14;
    fd = sc(2,"/writeback-sync",2|010000,0); // O_DSYNC uses the same durability path.
    if (fd < 0 || sc(1,fd,output,sizeof(output)) != sizeof(output) || sc(3,fd,0,0) != 0) return 16;
    fd = sc(2,"/",0200000,0); // O_DIRECTORY: parent-directory synchronization.
    if (fd < 0 || sc(74,fd,0,0) != 0 || sc(3,fd,0,0) != 0) return 17;
    return 0;
}

__asm__(".global _start\n_start:\nxor %ebp,%ebp\nand $-16,%rsp\ncall test_main\nmov %rax,%rdi\nmov $60,%eax\nsyscall\nud2\n");
