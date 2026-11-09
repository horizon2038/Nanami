//! Real path translation; unrelated pathname fetching and virtual lookup mocked.
#![allow(dead_code)]
type Word = usize;
const EINVAL: i32 = 22;
const ENAMETOOLONG: i32 = 36;
const EROFS: i32 = 30;
const ESRCH: i32 = 3;
const LINUX_AT_FDCWD: usize = (-100isize) as usize;
const LINUX_CWD_MAX: usize = 128;
mod abi { pub const ALTER_PATH_MAX: usize = 256; }
#[path = "../../nanami/servers/apps/alter/shared/src/common/context.rs"]
pub mod context;
mod common { pub use crate::context; }
#[derive(Clone, Copy)]
enum Personality { Linux, FreeBsd }
mod personality {
    pub fn root(personality: super::Personality) -> &'static [u8] {
        match personality {
            super::Personality::Linux => b"/alter/linux",
            super::Personality::FreeBsd => b"/alter/freebsd",
        }
    }
}
struct Process {
    rootfs: context::RootfsContext,
    personality: Personality,
    cwd: [u8; LINUX_CWD_MAX],
    cwd_len: usize,
}
struct Runtime { posix_shm: Word, posix_shm_size: Word, process: Process, exists: bool }
impl Runtime {
    fn managed_process(&self, _: Word) -> Option<&Process> {
        self.exists.then_some(&self.process)
    }
}
struct VirtualNode;
fn graphics_enabled(_: &Runtime, _: Word) -> bool { false }
mod virtual_fs {
    pub fn lookup(_: &[u8], _: bool) -> Option<super::VirtualNode> { None }
}
mod procfs {
    pub fn lookup(_: &super::Runtime, _: usize, _: &[u8]) -> Option<()> { None }
    pub fn node(_: ()) -> Option<super::VirtualNode> { None }
}
mod namespace {
    pub fn resolve_at(_: &mut super::Runtime, _: usize, _: usize, _: usize, _: bool) -> Result<usize, i32> {
        unreachable!("only translation is exercised")
    }
}
mod path_string {
    pub fn read_c_string(_: &mut super::Runtime, _: usize, _: usize) -> Result<usize, i32> {
        unreachable!("only translation is exercised")
    }
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/paths.rs"]
mod paths;

fn component(path: &[u8], prefix: &[u8]) -> bool {
    path.starts_with(prefix) && (path.len() == prefix.len() || path[prefix.len()] == b'/')
}
fn virtual_path(path: &[u8]) -> bool {
    [b"/dev".as_slice(), b"/proc", b"/sys"].iter().any(|prefix| component(path, prefix))
}
// The original staging-buffer algorithm, retaining mutation before /temp retry
// errors and every capacity/error check. Compare the whole buffer, not just a
// successful prefix, so error-side partial changes and NUL bytes stay visible.
fn original(rootfs: &context::RootfsContext, persona: Personality, exists: bool,
    buffer: &mut [u8], capacity: usize, offset: usize, len: usize) -> Result<usize, i32> {
    if len == 0 { return Err(EINVAL); }
    if offset.checked_add(len).and_then(|n| n.checked_add(1)).is_none_or(|n| n > capacity) {
        return Err(ENAMETOOLONG);
    }
    if !exists { return Err(ESRCH); }
    let path = buffer[offset..offset + len].to_vec();
    if rootfs.root_len != 0 {
        if virtual_path(&path) { return Ok(len); }
        let mut host = [0; abi::ALTER_PATH_MAX];
        let n = rootfs.host_path(&path, &mut host).ok_or(ENAMETOOLONG)?;
        if offset.checked_add(n + 1).is_none_or(|end| end > capacity) { return Err(ENAMETOOLONG); }
        buffer[offset..offset + n].copy_from_slice(&host[..n]);
        buffer[offset + n] = 0;
        return Ok(n);
    }
    if path[0] != b'/' || virtual_path(&path) { return Ok(len); }
    if component(&path, b"/temp") {
        let n = 4 + len - 5;
        buffer[offset + 4..offset + n].copy_from_slice(&path[5..]);
        buffer[offset..offset + 4].copy_from_slice(b"/tmp");
        buffer[offset + n] = 0;
        return original(rootfs, persona, exists, buffer, capacity, offset, n);
    }
    let root = personality::root(persona);
    if component(&path, root) { return Ok(len); }
    if len >= LINUX_CWD_MAX { return Err(ENAMETOOLONG); }
    let suffix = if len == 1 { &[][..] } else { &path[..] };
    let n = root.len() + suffix.len();
    if offset.checked_add(n).and_then(|n| n.checked_add(1)).is_none_or(|end| end > capacity) {
        return Err(ENAMETOOLONG);
    }
    buffer[offset..offset + root.len()].copy_from_slice(root);
    buffer[offset + root.len()..offset + n].copy_from_slice(suffix);
    buffer[offset + n] = 0;
    Ok(n)
}

#[test]
fn translation_matches_staging_oracle_including_all_error_bytes_and_guards() {
    let mut inputs: Vec<Vec<u8>> = [b"".as_slice(), b"/", b"x", b"/bin/x", b"/temp", b"/temp/x",
        b"/tmp/x", b"/dev", b"/dev/x", b"/proc/x", b"/sys/x", b"/device", b"/alter/linux",
        b"/alter/linux/x", b"/alter/linuxish", b"/alter/freebsd/x", b"//x", b"/x/../y", b"/x\0y"]
        .iter().map(|path| path.to_vec()).collect();
    for len in [1, 7, 126, 127, 128, 129, 254, 255, 256, 257, 300] {
        let mut path = vec![b'a'; len];
        path[0] = b'/';
        inputs.push(path);
    }
    let mut cases = 0;
    for persona in [Personality::Linux, Personality::FreeBsd] {
        for private in [false, true] {
            let rootfs = if private {
                context::RootfsContext::new(b"/bundle/rootfs", b"/bin/x", false).unwrap()
            } else { context::RootfsContext::DEFAULT };
            for exists in [false, true] {
                for input in &inputs {
                    for offset in [0, 1, 7, 31] {
                        for alignment in [0, 1, 7] {
                            for capacity in [0, 1, 7, 32, 127, 128, 256, 512, 1024] {
                                let start = 16 + alignment;
                                let mut storage = vec![0xa5; 2048];
                                storage[start + offset..start + offset + input.len()].copy_from_slice(input);
                                let mut expected = storage.clone();
                                let result = original(&rootfs, persona, exists, &mut expected[start..], capacity, offset, input.len());
                                let mut runtime = Runtime { posix_shm: storage.as_mut_ptr() as usize + start,
                                    posix_shm_size: capacity, exists,
                                    process: Process { rootfs, personality: persona, cwd: [0; LINUX_CWD_MAX], cwd_len: 0 } };
                                let actual = paths::translate_guest_path_at(&mut runtime, 1, offset, input.len());
                                assert_eq!(actual, result, "private={private} exists={exists} input={input:?} offset={offset} capacity={capacity}");
                                assert_eq!(storage, expected, "private={private} input={input:?} offset={offset} capacity={capacity}");
                                cases += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    println!("PATH-PREFIX-IN-PLACE full-buffer-oracle-cases={cases}");
}

fn lexical_reference(input: &[u8], process: Option<(&[u8], usize)>) -> Result<Vec<u8>, i32> {
    if input.len() >= LINUX_CWD_MAX { return Err(ENAMETOOLONG); }
    let mut output = if input.first() == Some(&b'/') {
        vec![b'/']
    } else {
        let (cwd, len) = process.ok_or(ESRCH)?;
        if len == 0 || len >= LINUX_CWD_MAX { return Err(EINVAL); }
        cwd[..len].to_vec()
    };
    for part in input.split(|byte| *byte == b'/') {
        if part.is_empty() || part == b"." { continue; }
        if part == b".." {
            if output.len() <= 1 {
                output.clear();
                output.push(b'/');
                continue;
            }
            while output.len() > 1 && output.last() == Some(&b'/') { output.pop(); }
            while output.len() > 1 && output.last() != Some(&b'/') { output.pop(); }
            if output.len() > 1 { output.pop(); }
            if output.is_empty() { output.push(b'/'); }
            continue;
        }
        if output.is_empty() { output.push(b'/'); }
        if output.len() != 1 {
            if output.len() + 1 >= LINUX_CWD_MAX { return Err(ENAMETOOLONG); }
            output.push(b'/');
        }
        if output.len() + part.len() >= LINUX_CWD_MAX { return Err(ENAMETOOLONG); }
        output.extend_from_slice(part);
    }
    if output.is_empty() { output.push(b'/'); }
    Ok(output)
}

#[test]
fn lexical_borrow_preserves_relative_cwd_errors_and_entire_output_buffer() {
    let mut inputs: Vec<Vec<u8>> = [b"".as_slice(), b"/", b"x", b".", b"..", b"/../..", b"a/../b",
        b"/a//b/./../c/", b"////", b"./a/../../b", b"/a\0b", b"/./../x"]
        .iter().map(|path| path.to_vec()).collect();
    for len in [126, 127, 128, 129] {
        inputs.push(vec![b'a'; len]);
        let mut absolute = vec![b'a'; len];
        absolute[0] = b'/';
        inputs.push(absolute);
    }
    let mut random = 0x2f31_914du32;
    for _ in 0..2048 {
        random = random.wrapping_mul(1664525).wrapping_add(1013904223);
        let len = (random >> 16) as usize % 130;
        let mut path = Vec::new();
        for _ in 0..len {
            random = random.wrapping_mul(1664525).wrapping_add(1013904223);
            path.push([b'/', b'.', b'a', b'b', 0][(random >> 16) as usize % 5]);
        }
        inputs.push(path);
    }
    let mut cases = 0;
    for cwd in [b"/".as_slice(), b"/base/dir", b"/base/dir///", b"x"] {
        for cwd_len in [0, cwd.len(), 127, 128, usize::MAX] {
            for exists in [false, true] {
                for input in &inputs {
                    for alignment in [0, 1, 7] {
                        let start = 16 + alignment;
                        let mut process = Process { rootfs: context::RootfsContext::DEFAULT,
                            personality: Personality::Linux, cwd: [b'x'; LINUX_CWD_MAX], cwd_len };
                        process.cwd[..cwd.len()].copy_from_slice(cwd);
                        let reference = lexical_reference(input, exists.then_some((&process.cwd[..], cwd_len)));
                        let mut storage = vec![0xa5; 512];
                        storage[start..start + input.len()].copy_from_slice(input);
                        let mut expected = storage.clone();
                        let expected_result = reference.map(|bytes| {
                            expected[start..start + bytes.len()].copy_from_slice(&bytes);
                            expected[start + bytes.len()] = 0;
                            bytes.len()
                        });
                        let mut runtime = Runtime { posix_shm: storage.as_mut_ptr() as usize + start,
                            posix_shm_size: 512 - start, process, exists };
                        let result = paths::resolve_lexical_shm_path(&mut runtime, 1, input.len());
                        assert_eq!(result, expected_result, "cwd={cwd:?} len={cwd_len} input={input:?}");
                        assert_eq!(storage, expected, "cwd={cwd:?} len={cwd_len} input={input:?}");
                        cases += 1;
                    }
                }
            }
        }
    }
    println!("PATH-LEXICAL-BORROW full-buffer-reference-cases={cases}");
}
