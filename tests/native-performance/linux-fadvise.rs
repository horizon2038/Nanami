//! Real fadvise handler, with only descriptor lookup replaced by a fixture.
#![allow(dead_code)]
type Word = usize;
const EBADF: i32 = 9;
const EINVAL: i32 = 22;
const ESPIPE: i32 = 29;
#[derive(Clone, Copy)]
enum LinuxFileKind {
    Posix,
    PipeRead,
    PipeWrite,
    MemoryFile,
    SocketUnix,
    VirtualDirectory,
}
#[derive(Clone, Copy)]
struct LinuxFile {
    kind: LinuxFileKind,
    resource: Word,
}
impl LinuxFile {
    fn has_posix_backend(self) -> bool {
        matches!(self.kind, LinuxFileKind::Posix)
    }
}
struct Runtime(LinuxFile);
impl Runtime {
    fn linux_file(&self, pid: Word, fd: Word) -> Option<LinuxFile> {
        (pid == 7 && fd == 3).then_some(self.0)
    }
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/fadvise.rs"]
mod fadvise;
fn runtime(kind: LinuxFileKind) -> Runtime {
    Runtime(LinuxFile { kind, resource: 0 })
}
#[test]
fn all_six_hints_accept_zero_large_and_signed_ranges_without_io() {
    for kind in [
        LinuxFileKind::Posix,
        LinuxFileKind::MemoryFile,
        LinuxFileKind::VirtualDirectory,
        LinuxFileKind::SocketUnix,
    ] {
        let runtime = runtime(kind);
        for advice in 0..6 {
            for (offset, length) in [
                (0, 0),
                (0, 4096),
                (Word::MAX, 4096),
                (i64::MAX as Word, i64::MAX as Word),
            ] {
                assert_eq!(
                    fadvise::advise(&runtime, 7, 3, offset, length, advice),
                    Ok(0)
                );
            }
        }
    }
}
#[test]
fn bad_descriptors_pipes_and_invalid_arguments_have_linux_errors() {
    let regular = runtime(LinuxFileKind::Posix);
    assert_eq!(fadvise::advise(&regular, 7, 4, 0, 0, 0), Err(EBADF));
    assert_eq!(fadvise::advise(&regular, 8, 3, 0, 0, 0), Err(EBADF));
    assert_eq!(
        fadvise::advise(&regular, 7, 3, 0, Word::MAX, 0),
        Err(EINVAL)
    );
    for advice in [6, Word::MAX] {
        assert_eq!(fadvise::advise(&regular, 7, 3, 0, 0, advice), Err(EINVAL));
    }
    for kind in [LinuxFileKind::PipeRead, LinuxFileKind::PipeWrite] {
        assert_eq!(
            fadvise::advise(&runtime(kind), 7, 3, 0, Word::MAX, 99),
            Err(ESPIPE)
        );
    }
    let path = Runtime(LinuxFile {
        kind: LinuxFileKind::Posix,
        resource: 0x20_0000,
    });
    assert_eq!(fadvise::advise(&path, 7, 3, 0, 0, 0), Err(EBADF));
}
#[test]
fn int_arguments_ignore_the_upper_register_bits() {
    let runtime = runtime(LinuxFileKind::Posix);
    assert_eq!(
        fadvise::advise(&runtime, 7, (1 << 32) | 3, 0, 0, (1 << 32) | 5),
        Ok(0)
    );
}
