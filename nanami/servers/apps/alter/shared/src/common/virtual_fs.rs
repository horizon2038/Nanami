use nanami_runtime::Word;

use crate::arch::{CPU_INFO, PROC_VERSION};

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum VirtualNode {
    Root = 1,
    Dev,
    DevInput,
    DevNull,
    DevZero,
    DevTty,
    DevKeyboard,
    DevMouse,
    DevFramebuffer,
    Proc,
    ProcSelf,
    ProcSelfExe,
    ProcVersion,
    ProcCpuInfo,
    ProcMemInfo,
    Sys,
    SysClass,
    SysClassInput,
    SysClassGraphics,
    Tmp,
    Temp,
    RootBin,
    RootEtc,
    RootUsr,
    RootSbin,
    ProcNet,
    ProcNetArp,
    ResolvConf,
    Hosts,
    DevPtmx,
    DevPts,
    DevConsole,
    DevPs2Mouse,
    ProcProcess,
    ProcTask,
    ProcStat,
    ProcStatus,
    ProcCmdline,
    ProcComm,
    ProcStatm,
    ProcUptime,
}

impl VirtualNode {
    pub const fn id(self) -> Word {
        self as Word
    }

    pub const fn from_id(id: Word) -> Option<Self> {
        match id {
            1 => Some(Self::Root),
            2 => Some(Self::Dev),
            3 => Some(Self::DevInput),
            4 => Some(Self::DevNull),
            5 => Some(Self::DevZero),
            6 => Some(Self::DevTty),
            7 => Some(Self::DevKeyboard),
            8 => Some(Self::DevMouse),
            9 => Some(Self::DevFramebuffer),
            10 => Some(Self::Proc),
            11 => Some(Self::ProcSelf),
            12 => Some(Self::ProcSelfExe),
            13 => Some(Self::ProcVersion),
            14 => Some(Self::ProcCpuInfo),
            15 => Some(Self::ProcMemInfo),
            16 => Some(Self::Sys),
            17 => Some(Self::SysClass),
            18 => Some(Self::SysClassInput),
            19 => Some(Self::SysClassGraphics),
            20 => Some(Self::Tmp),
            21 => Some(Self::Temp),
            22 => Some(Self::RootBin),
            23 => Some(Self::RootEtc),
            24 => Some(Self::RootUsr),
            25 => Some(Self::RootSbin),
            26 => Some(Self::ProcNet),
            27 => Some(Self::ProcNetArp),
            28 => Some(Self::ResolvConf),
            29 => Some(Self::Hosts),
            30 => Some(Self::DevPtmx),
            31 => Some(Self::DevPts),
            32 => Some(Self::DevConsole),
            33 => Some(Self::DevPs2Mouse),
            34 => Some(Self::ProcProcess),
            35 => Some(Self::ProcTask),
            36 => Some(Self::ProcStat),
            37 => Some(Self::ProcStatus),
            38 => Some(Self::ProcCmdline),
            39 => Some(Self::ProcComm),
            40 => Some(Self::ProcStatm),
            41 => Some(Self::ProcUptime),
            _ => None,
        }
    }

    pub const fn is_directory(self) -> bool {
        matches!(
            self,
            Self::Root
                | Self::Dev
                | Self::DevInput
                | Self::DevPts
                | Self::Proc
                | Self::ProcSelf
                | Self::ProcProcess
                | Self::ProcTask
                | Self::Sys
                | Self::SysClass
                | Self::SysClassInput
                | Self::SysClassGraphics
                | Self::Tmp
                | Self::Temp
                | Self::RootBin
                | Self::RootEtc
                | Self::RootUsr
                | Self::RootSbin
                | Self::ProcNet
        )
    }

    pub const fn is_regular_file(self) -> bool {
        matches!(
            self,
            Self::ProcSelfExe
                | Self::ProcVersion
                | Self::ProcCpuInfo
                | Self::ProcMemInfo
                | Self::ProcNetArp
                | Self::ProcStat
                | Self::ProcStatus
                | Self::ProcCmdline
                | Self::ProcComm
                | Self::ProcStatm
                | Self::ProcUptime
                | Self::ResolvConf
                | Self::Hosts
        )
    }
}

#[derive(Clone, Copy)]
pub struct DirectoryEntry {
    pub name: &'static [u8],
    pub node: VirtualNode,
}

// These mounts exist in every guest root. Ordinary root entries come from VFS.
const PRIVATE_ROOT_MOUNTS: &[DirectoryEntry] = &[
    DirectoryEntry {
        name: b"dev",
        node: VirtualNode::Dev,
    },
    DirectoryEntry {
        name: b"proc",
        node: VirtualNode::Proc,
    },
    DirectoryEntry {
        name: b"sys",
        node: VirtualNode::Sys,
    },
];

// /temp aliases /tmp only in the legacy personality root. A private OCI root
// may have its own /temp, or none; never invent or shadow that disk entry.
pub const ROOT_MOUNTS: &[DirectoryEntry] = &[
    PRIVATE_ROOT_MOUNTS[0],
    PRIVATE_ROOT_MOUNTS[1],
    PRIVATE_ROOT_MOUNTS[2],
    DirectoryEntry {
        name: b"temp",
        node: VirtualNode::Temp,
    },
];

pub fn root_mounts(private_root: bool) -> &'static [DirectoryEntry] {
    if private_root {
        PRIVATE_ROOT_MOUNTS
    } else {
        ROOT_MOUNTS
    }
}

pub fn lookup(path: &[u8], graphics_enabled: bool) -> Option<VirtualNode> {
    match path {
        b"/" => Some(VirtualNode::Root),
        b"/dev" => Some(VirtualNode::Dev),
        b"/dev/ptmx" | b"/dev/pts/ptmx" => Some(VirtualNode::DevPtmx),
        b"/dev/pts" => Some(VirtualNode::DevPts),
        b"/dev/input" => Some(VirtualNode::DevInput),
        b"/dev/null" => Some(VirtualNode::DevNull),
        b"/dev/zero" => Some(VirtualNode::DevZero),
        b"/dev/tty0" | b"/dev/tty1" | b"/dev/console" if graphics_enabled => {
            Some(VirtualNode::DevConsole)
        }
        b"/dev/input/mice" | b"/dev/psaux" | b"/dev/mouse" if graphics_enabled => {
            Some(VirtualNode::DevPs2Mouse)
        }
        b"/dev/tty" | b"/dev/console" | b"/dev/tty0" => Some(VirtualNode::DevTty),
        b"/dev/input/event0" => Some(VirtualNode::DevKeyboard),
        b"/dev/input/event1" => Some(VirtualNode::DevMouse),
        b"/dev/fb0" if graphics_enabled => Some(VirtualNode::DevFramebuffer),
        b"/proc" => Some(VirtualNode::Proc),
        b"/proc/self" => Some(VirtualNode::ProcSelf),
        b"/proc/self/exe" => Some(VirtualNode::ProcSelfExe),
        b"/proc/version" => Some(VirtualNode::ProcVersion),
        b"/proc/cpuinfo" => Some(VirtualNode::ProcCpuInfo),
        b"/proc/meminfo" => Some(VirtualNode::ProcMemInfo),
        b"/proc/uptime" => Some(VirtualNode::ProcUptime),
        b"/proc/net" | b"/proc/self/net" => Some(VirtualNode::ProcNet),
        b"/proc/net/arp" | b"/proc/self/net/arp" => Some(VirtualNode::ProcNetArp),
        b"/sys" => Some(VirtualNode::Sys),
        b"/sys/class" => Some(VirtualNode::SysClass),
        b"/sys/class/input" => Some(VirtualNode::SysClassInput),
        b"/sys/class/input/event0" => Some(VirtualNode::DevKeyboard),
        b"/sys/class/input/event1" => Some(VirtualNode::DevMouse),
        b"/sys/class/graphics" if graphics_enabled => Some(VirtualNode::SysClassGraphics),
        b"/sys/class/graphics/fb0" if graphics_enabled => Some(VirtualNode::DevFramebuffer),
        _ => None,
    }
}

pub fn directory_entry(
    directory: VirtualNode,
    index: usize,
    graphics_enabled: bool,
) -> Option<DirectoryEntry> {
    let entry = match directory {
        VirtualNode::Root => *ROOT_MOUNTS.get(index)?,
        VirtualNode::Dev => match index {
            0 => DirectoryEntry {
                name: b"input",
                node: VirtualNode::DevInput,
            },
            1 => DirectoryEntry {
                name: b"null",
                node: VirtualNode::DevNull,
            },
            2 => DirectoryEntry {
                name: b"zero",
                node: VirtualNode::DevZero,
            },
            3 => DirectoryEntry {
                name: b"tty",
                node: VirtualNode::DevTty,
            },
            4 => DirectoryEntry {
                name: b"ptmx",
                node: VirtualNode::DevPtmx,
            },
            5 => DirectoryEntry {
                name: b"pts",
                node: VirtualNode::DevPts,
            },
            6 if graphics_enabled => DirectoryEntry {
                name: b"fb0",
                node: VirtualNode::DevFramebuffer,
            },
            7 | 8 if graphics_enabled => DirectoryEntry {
                name: if index == 7 { b"tty0" } else { b"tty1" },
                node: VirtualNode::DevConsole,
            },
            _ => return None,
        },
        VirtualNode::DevInput => match index {
            0 => DirectoryEntry {
                name: b"event0",
                node: VirtualNode::DevKeyboard,
            },
            1 => DirectoryEntry {
                name: b"event1",
                node: VirtualNode::DevMouse,
            },
            2 if graphics_enabled => DirectoryEntry {
                name: b"mice",
                node: VirtualNode::DevPs2Mouse,
            },
            _ => return None,
        },
        VirtualNode::Proc => match index {
            0 => DirectoryEntry {
                name: b"self",
                node: VirtualNode::ProcSelf,
            },
            1 => DirectoryEntry {
                name: b"version",
                node: VirtualNode::ProcVersion,
            },
            2 => DirectoryEntry {
                name: b"cpuinfo",
                node: VirtualNode::ProcCpuInfo,
            },
            3 => DirectoryEntry {
                name: b"meminfo",
                node: VirtualNode::ProcMemInfo,
            },
            4 => DirectoryEntry {
                name: b"net",
                node: VirtualNode::ProcNet,
            },
            5 => DirectoryEntry {
                name: b"uptime",
                node: VirtualNode::ProcUptime,
            },
            _ => return None,
        },
        VirtualNode::ProcSelf => match index {
            0 => DirectoryEntry {
                name: b"exe",
                node: VirtualNode::ProcSelfExe,
            },
            1 => DirectoryEntry {
                name: b"net",
                node: VirtualNode::ProcNet,
            },
            _ => return None,
        },
        VirtualNode::ProcNet => match index {
            0 => DirectoryEntry {
                name: b"arp",
                node: VirtualNode::ProcNetArp,
            },
            _ => return None,
        },
        VirtualNode::Sys => match index {
            0 => DirectoryEntry {
                name: b"class",
                node: VirtualNode::SysClass,
            },
            _ => return None,
        },
        VirtualNode::SysClass => match index {
            0 => DirectoryEntry {
                name: b"input",
                node: VirtualNode::SysClassInput,
            },
            1 if graphics_enabled => DirectoryEntry {
                name: b"graphics",
                node: VirtualNode::SysClassGraphics,
            },
            _ => return None,
        },
        VirtualNode::SysClassInput => match index {
            0 => DirectoryEntry {
                name: b"event0",
                node: VirtualNode::DevKeyboard,
            },
            1 => DirectoryEntry {
                name: b"event1",
                node: VirtualNode::DevMouse,
            },
            _ => return None,
        },
        VirtualNode::SysClassGraphics if graphics_enabled => match index {
            0 => DirectoryEntry {
                name: b"fb0",
                node: VirtualNode::DevFramebuffer,
            },
            _ => return None,
        },
        _ => return None,
    };
    Some(entry)
}

pub fn static_file(node: VirtualNode) -> Option<&'static [u8]> {
    match node {
        VirtualNode::ProcVersion => Some(PROC_VERSION),
        VirtualNode::ProcCpuInfo => Some(CPU_INFO),
        VirtualNode::Hosts => Some(b"127.0.0.1 localhost\n"),
        _ => None,
    }
}

/// Defaults only: a real rootfs file always takes precedence.
pub fn fallback_file(path: &[u8]) -> Option<VirtualNode> {
    match path {
        b"/etc/resolv.conf" => Some(VirtualNode::ResolvConf),
        b"/etc/hosts" => Some(VirtualNode::Hosts),
        _ => None,
    }
}
