use crate::model::{Monitor, Tab};
use alloc::{format, string::String};
use nanami_ui::{Canvas, Font, Rect, ACCENT, CHROME, ERROR, INK, MUTED, PAPER, RULE};
#[derive(Default)]
pub struct System {
    pub architecture: String,
    pub platform: String,
    pub kernel: String,
    pub os: String,
}
pub fn draw(c: &mut Canvas<'_>, font: &Font, m: &Monitor, system: &System) {
    let w = c.width;
    let h = c.height;
    c.rect(Rect::new(0, 0, w, h), PAPER);
    c.rect(Rect::new(0, 0, w, 70), CHROME);
    for (x, width, text, active) in [
        (8, 90, if m.paused { "Resume" } else { "Pause" }, false),
        (106, 62, "0.5 s", m.interval == 0),
        (174, 54, "1 s", m.interval == 1),
        (234, 54, "2 s", m.interval == 2),
        (302, 88, "Refresh", false),
        (398, 90, "Reset", false),
    ] {
        if x + width <= w {
            c.button(font, Rect::new(x as i32, 3, width, 28), text, active);
        }
    }
    for (x, width, label, tab) in [
        (8, 124, "Memory", Tab::Memory),
        (132, 144, "Processes", Tab::Processes),
        (276, 124, "System", Tab::System),
    ] {
        if x + width <= w {
            c.button(
                font,
                Rect::new(x as i32, 38, width, 28),
                label,
                m.tab == tab,
            );
        }
    }
    c.rect(Rect::new(0, 69, w, 1), RULE);
    let bottom = h.saturating_sub(26);
    c.rect(Rect::new(0, bottom as i32, w, 26), CHROME);
    c.text(
        font,
        12,
        bottom as i32 + 5,
        if m.paused { "Paused" } else { "Sampling" },
        MUTED,
        w,
    );
    let count = format!("{} / 120 samples", m.history.len);
    let x = w.saturating_sub(count.len() * 9 + 12);
    c.text(
        font,
        x as i32,
        bottom as i32 + 5,
        &count,
        MUTED,
        w.saturating_sub(x),
    );
    c.clip = Rect::new(0, 72, w, bottom.saturating_sub(72));
    if m.tab == Tab::System {
        row(c, font, 90, "Architecture", &system.architecture);
        row(c, font, 120, "Platform", &system.platform);
        row(c, font, 150, "Nanami", &system.os);
        row(c, font, 180, "A9N", &system.kernel);
        if let Some(s) = m.history.latest() {
            if let Some((count, mask)) = s.cores {
                row(c, font, 220, "Online cores", &format!("{}", count));
                row(c, font, 250, "Assigned mask", &format!("{:#018x}", mask));
                let columns = (w.saturating_sub(40) / 70).max(1);
                for core in 0..count.min(64) {
                    let x = 20 + core % columns * 70;
                    let y = 286 + core / columns * 26;
                    c.text(
                        font,
                        x as i32,
                        y as i32,
                        &format!(
                            "{}{}",
                            core,
                            if mask & (1 << core) != 0 { " *" } else { "" }
                        ),
                        INK,
                        66,
                    );
                }
                let y = 286 + count.min(64).div_ceil(columns) * 26 + 10;
                c.text(
                    font,
                    20,
                    y as i32,
                    "* Has assigned processes; not CPU utilization",
                    MUTED,
                    w.saturating_sub(40),
                );
            }
            row(
                c,
                font,
                bottom.saturating_sub(72).max(360) as i32,
                "Uptime",
                &format_time(s.at_ms),
            );
        }
        return;
    }
    let memory = m.tab == Tab::Memory;
    let latest = m.history.latest();
    let heading = match latest {
        Some(s) if memory => s.memory.map(|(total, free)| {
            format!(
                "{} used / {} total   {}%",
                bytes(total.saturating_sub(free)),
                bytes(total),
                s.percentage().unwrap_or(0)
            )
        }),
        Some(s) => s
            .processes
            .map(|(alive, exited)| format!("{} alive   {} exited (retained)", alive, exited)),
        None => None,
    };
    c.text(
        font,
        20,
        86,
        heading.as_deref().unwrap_or("Statistics unavailable"),
        if heading.is_some() { INK } else { ERROR },
        w.saturating_sub(40),
    );
    let graph_height = bottom.saturating_sub(310).clamp(60, 180);
    if !memory {
        c.text(font, 58, 108, "Alive", ACCENT, 90);
        c.text(font, 158, 108, "Exited", MUTED, 90);
    }
    graph(
        c,
        font,
        m,
        Rect::new(58, 128, w.saturating_sub(82), graph_height),
    );
    let y = 128 + graph_height as i32 + 42;
    if memory {
        if let Some(s) = latest {
            if let Some((total, free)) = s.memory {
                row(c, font, y, "Free physical", &bytes(free));
                let delta = m
                    .history
                    .previous()
                    .and_then(|p| p.used())
                    .map(|p| total.saturating_sub(free) as i128 - p as i128);
                row(
                    c,
                    font,
                    y + 28,
                    "Change / sample",
                    &delta.map(signed_bytes).unwrap_or_else(|| "--".into()),
                );
            }
        }
        if let Some((min, max, average)) = m.history.memory_stats() {
            row(
                c,
                font,
                y + 56,
                "History min / max",
                &format!("{} / {}", bytes(min), bytes(max)),
            );
            row(c, font, y + 84, "History average", &bytes(average));
        }
        c.text(
            font,
            20,
            y + 124,
            "Source: Alpha physical allocator, not process RSS",
            MUTED,
            w.saturating_sub(40),
        );
    } else {
        if let Some((alive, exited)) = latest.and_then(|s| s.processes) {
            row(
                c,
                font,
                y,
                "Tracked total",
                &format!("{}", alive.saturating_add(exited)),
            );
            let previous = m.history.previous().and_then(|s| s.processes);
            row(
                c,
                font,
                y + 28,
                "Alive change",
                &previous
                    .map(|(a, _)| format!("{:+}", alive as i128 - a as i128))
                    .unwrap_or_else(|| "--".into()),
            );
            row(
                c,
                font,
                y + 56,
                "Exited change",
                &previous
                    .map(|(_, e)| format!("{:+}", exited as i128 - e as i128))
                    .unwrap_or_else(|| "--".into()),
            );
        }
        c.text(
            font,
            20,
            y + 100,
            "Alive includes blocked tasks. Exited excludes reaped tasks.",
            MUTED,
            w.saturating_sub(40),
        );
        c.text(
            font,
            20,
            y + 126,
            "Per-process CPU time is not exposed by A9N.",
            MUTED,
            w.saturating_sub(40),
        );
    }
}
fn row(c: &mut Canvas<'_>, font: &Font, y: i32, label: &str, value: &str) {
    let split = c.width.min(230);
    c.text(font, 20, y, label, MUTED, split.saturating_sub(28));
    c.text(
        font,
        split as i32,
        y,
        value,
        INK,
        c.width.saturating_sub(split + 12),
    );
}
fn graph(c: &mut Canvas<'_>, font: &Font, m: &Monitor, area: Rect) {
    if area.w < 2 || area.h < 2 {
        return;
    }
    let memory = m.tab == Tab::Memory;
    let scale = if memory {
        100
    } else {
        m.history.process_scale()
    };
    for (fraction, value) in [(0, scale), (1, scale / 2), (2, 0)] {
        let y = area.y + (area.h - 1) as i32 * fraction / 2;
        c.line(area.x, y, area.x + area.w as i32 - 1, y, RULE);
        c.text(font, 8, y - 5, &format!("{}", value), MUTED, 48);
    }
    if m.history.len == 0 {
        return;
    }
    let oldest = m.history.get(0).at_ms;
    let newest = m.history.get(m.history.len - 1).at_ms;
    for series in 0..if memory { 1 } else { 2 } {
        let mut previous = None;
        for i in 0..m.history.len {
            let s = m.history.get(i);
            let value = if memory {
                s.percentage()
            } else {
                s.processes.map(|(a, b)| if series == 0 { a } else { b })
            };
            if let Some(value) = value {
                let x = area.x
                    + ((s.at_ms.saturating_sub(oldest) as u128 * (area.w - 1) as u128)
                        / newest.saturating_sub(oldest).max(1) as u128)
                        as i32;
                let y = area.y + (area.h - 1) as i32
                    - ((value.min(scale) as u128 * (area.h - 1) as u128) / scale as u128) as i32;
                let color = if series == 0 { ACCENT } else { MUTED };
                if let Some((px, py)) = previous {
                    c.line(px, py, x, y, color);
                } else {
                    c.pixel(x, y, color);
                }
                previous = Some((x, y));
            } else {
                previous = None;
            }
        }
    }
    c.text(
        font,
        area.x,
        area.y + area.h as i32 + 8,
        &format!(
            "-{}.{:01} s",
            (newest - oldest) / 1000,
            (newest - oldest) % 1000 / 100
        ),
        MUTED,
        area.w,
    );
    c.text(
        font,
        area.x + area.w as i32 - 36,
        area.y + area.h as i32 + 8,
        "now",
        MUTED,
        36,
    );
}
pub fn bytes(value: u64) -> String {
    if value < 1024 {
        return format!("{} B", value);
    }
    if value < 1048576 {
        return format!("{}.{:01} KiB", value / 1024, value % 1024 * 10 / 1024);
    }
    format!(
        "{}.{:01} MiB",
        value / 1048576,
        value % 1048576 * 10 / 1048576
    )
}
fn signed_bytes(value: i128) -> String {
    format!(
        "{}{}",
        if value < 0 { "-" } else { "+" },
        bytes(value.unsigned_abs().min(u64::MAX as u128) as u64)
    )
}
fn format_time(ms: u64) -> String {
    let s = ms / 1000;
    format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}
