use crate::framebuffer_size::FramebufferSize;

pub const ALTER_LAUNCH_MAX_ARGS: usize = 8;

pub struct Launch {
    pub trace: bool,
    pub diagnostics: bool,
    pub graphics: bool,
    pub framebuffer_size: Option<FramebufferSize>,
    pub os: AlterOs,
    pub first_arg: usize,
    pub argc: usize,
}

#[derive(Clone, Copy)]
pub enum AlterOs {
    Linux,
    FreeBsd,
}

impl AlterOs {
    pub fn service_name(self) -> &'static str {
        match self {
            Self::Linux => "alter-linux",
            Self::FreeBsd => "alter-freebsd",
        }
    }

    pub fn unavailable_message(self) -> &'static [u8] {
        match self {
            Self::Linux => b"alter: alter-linux unavailable",
            Self::FreeBsd => b"alter: alter-freebsd unavailable",
        }
    }
}

pub fn parse_cli<'a>(argc: usize, arg: impl Fn(usize) -> Option<&'a [u8]>) -> Result<Launch, ()> {
    let mut index = 1usize;
    let mut trace = false;
    let mut diagnostics = false;
    let mut graphics = false;
    let mut framebuffer_size = None;
    let mut os = AlterOs::Linux;
    while index < argc {
        match arg(index).ok_or(())? {
            b"-t" | b"--strace" => trace = true,
            b"-d" | b"--diagnostics" => diagnostics = true,
            b"-g" | b"--graphics" => graphics = true,
            b"--fb-size" => {
                if framebuffer_size.is_some() {
                    return Err(());
                }
                index += 1;
                framebuffer_size = Some(FramebufferSize::parse(arg(index).ok_or(())?).ok_or(())?);
            }
            b"-os" => {
                index += 1;
                os = match arg(index).ok_or(())? {
                    b"linux" => AlterOs::Linux,
                    b"freebsd" => AlterOs::FreeBsd,
                    _ => return Err(()),
                };
            }
            _ => break,
        }
        index += 1;
    }
    if index + 1 < argc {
        match arg(index).ok_or(())? {
            b"linux" => {
                os = AlterOs::Linux;
                index += 1;
            }
            b"freebsd" => {
                os = AlterOs::FreeBsd;
                index += 1;
            }
            _ => {}
        }
    }
    if index >= argc
        || argc - index > ALTER_LAUNCH_MAX_ARGS
        || (framebuffer_size.is_some() && !graphics)
    {
        return Err(());
    }
    Ok(Launch {
        trace,
        diagnostics,
        graphics,
        framebuffer_size,
        os,
        first_arg: index,
        argc: argc - index,
    })
}
