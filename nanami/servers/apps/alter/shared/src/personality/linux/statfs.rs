//! Filesystem identity and live ext2 counters, never invented disk capacity.
use super::*;
use nanami_client::vfs::FilesystemStats;

fn virtual_stats(kind: u64) -> FilesystemStats {
    FilesystemStats {
        kind,
        block_size: LINUX_PAGE_SIZE as u64,
        name_max: 255,
        ..FilesystemStats::default()
    }
}

fn node_stats(node: VirtualNode) -> FilesystemStats {
    let kind = match node {
        VirtualNode::Sys
        | VirtualNode::SysClass
        | VirtualNode::SysClassInput
        | VirtualNode::SysClassGraphics => 0x62656572,
        VirtualNode::DevPts | VirtualNode::DevPtmx => 0x1cd1,
        VirtualNode::Proc
        | VirtualNode::ProcSelf
        | VirtualNode::ProcSelfExe
        | VirtualNode::ProcVersion
        | VirtualNode::ProcCpuInfo
        | VirtualNode::ProcMemInfo
        | VirtualNode::ProcNet
        | VirtualNode::ProcNetArp
        | VirtualNode::ProcProcess
        | VirtualNode::ProcTask
        | VirtualNode::ProcStat
        | VirtualNode::ProcStatus
        | VirtualNode::ProcCmdline
        | VirtualNode::ProcComm
        | VirtualNode::ProcStatm
        | VirtualNode::ProcUptime => 0x9fa0,
        _ => 0x01021994, // RAM-backed synthetic namespace / devices.
    };
    virtual_stats(kind)
}

fn write(runtime: &Runtime, pid: Word, output: Word, stats: FilesystemStats) -> Result<Word, i32> {
    let mut bytes = [0u8; 120];
    let fields = [
        stats.kind,
        stats.block_size,
        stats.blocks,
        stats.free_blocks,
        stats.available_blocks,
        stats.files,
        stats.free_files,
        stats.fsid,
        stats.name_max,
        stats.block_size,
        stats.flags | 0x20 | runtime.managed_process(pid).ok_or(ESRCH)?.rootfs.readonly as u64,
    ];
    for (index, field) in fields.iter().enumerate() {
        bytes[index * 8..index * 8 + 8].copy_from_slice(&field.to_le_bytes());
    }
    copyout_from(runtime, pid, output, bytes.as_ptr() as Word, bytes.len())?;
    Ok(0)
}

pub(super) fn fstatfs(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    output: Word,
) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    let stats = if file.has_posix_backend() {
        posix::posix_fstatfs(runtime.posix_port, file.posix_fd, runtime.posix_shm)
            .map_err(map_request_error)?
    } else {
        match file.kind {
            LinuxFileKind::VirtualFile | LinuxFileKind::VirtualDirectory => {
                node_stats(procfs::node(file.resource).ok_or(EBADF)?)
            }
            LinuxFileKind::PtyMaster | LinuxFileKind::PtySlave => virtual_stats(0x1cd1),
            LinuxFileKind::PipeRead | LinuxFileKind::PipeWrite => virtual_stats(0x50495045),
            LinuxFileKind::SocketUnix
            | LinuxFileKind::SocketTcp
            | LinuxFileKind::SocketTcpListener
            | LinuxFileKind::SocketUdp
            | LinuxFileKind::SocketIcmp
            | LinuxFileKind::SocketNetlink => virtual_stats(0x534f434b),
            LinuxFileKind::MemoryFile
            | LinuxFileKind::Terminal
            | LinuxFileKind::Framebuffer
            | LinuxFileKind::EvdevKeyboard
            | LinuxFileKind::EvdevMouse => virtual_stats(0x01021994),
            LinuxFileKind::EventFd
            | LinuxFileKind::TimerFd
            | LinuxFileKind::SignalFd
            | LinuxFileKind::Epoll
            | LinuxFileKind::IoUring => virtual_stats(0x09041934),
            _ => return Err(EBADF),
        }
    };
    write(runtime, pid, output, stats)
}

pub(super) fn statfs(
    runtime: &mut Runtime,
    pid: Word,
    path: Word,
    output: Word,
) -> Result<Word, i32> {
    let len = resolve_path(runtime, pid, path)?;
    if is_linux_virtual_path(runtime.posix_shm, len) {
        let node = current_virtual_node(runtime, pid, len).ok_or(ENOENT)?;
        return write(runtime, pid, output, node_stats(node));
    }
    let len = translate_guest_path_for_vfs(runtime, pid, len)?;
    // A temporary backend handle, not a guest FD: statfs must still work
    // when every entry in the guest's descriptor table is occupied.
    let fd = posix::posix_open(runtime.posix_port, 0, len, 0).map_err(map_path_request_error)?;
    let result =
        posix::posix_fstatfs(runtime.posix_port, fd, runtime.posix_shm).map_err(map_request_error);
    let _ = posix::posix_close(runtime.posix_port, fd);
    write(runtime, pid, output, result?)
}
