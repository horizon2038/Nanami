#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options<'a> {
    pub port: u16,
    pub file: &'a str,
    pub css: &'a str,
}
impl<'a> Options<'a> {
    pub fn parse(mut arg: impl FnMut(usize) -> Option<&'a str>) -> Result<Self, &'static str> {
        let mut options = Self {
            port: 80,
            file: "/www/index.html",
            css: "/www/style.css",
        };
        let mut index = 1;
        while let Some(option) = arg(index) {
            let value = arg(index + 1).ok_or("Missing option value")?;
            match option {
                "--port" => {
                    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                        return Err("Invalid TCP port");
                    }
                    options.port = value.parse().map_err(|_| "Invalid TCP port")?;
                    if options.port == 0 {
                        return Err("TCP port must be between 1 and 65535");
                    }
                }
                "--file" => options.file = value,
                "--css" => options.css = value,
                _ => return Err("Unknown option"),
            }
            index += 2;
        }
        for path in [options.file, options.css] {
            if path.is_empty() || path.len() > 255 || path.contains('\0') {
                return Err("Invalid file path");
            }
        }
        Ok(options)
    }
}
