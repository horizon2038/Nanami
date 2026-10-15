//! Default spawn priorities. Explicit boot/rootfs priority overrides are unchanged.
use nun::Word;

const PROCESS_PRIORITY_LOW: Word = 4;
const PROCESS_PRIORITY_BACKGROUND_CLIENT: Word = 8;
const PROCESS_PRIORITY_CLIENT: Word = 16;
const PROCESS_PRIORITY_INTERACTIVE_CLIENT: Word = 18;
const PROCESS_PRIORITY_BACKGROUND_SERVER: Word = 21;
const PROCESS_PRIORITY_GUI_SERVER: Word = 24;
const PROCESS_PRIORITY_INPUT_SERVER: Word = 28;
const PROCESS_PRIORITY_TIMER_SERVER: Word = 30;

pub(super) fn process_priority_for_image(image_name: &str) -> Word {
    let name = image_name
        .rsplit_once('/')
        .map_or(image_name, |(_, name)| name);
    let name = name.strip_suffix(".elf").unwrap_or(name);
    match name {
        // Timer must preempt clients promptly; animation and network timeouts depend on it.
        "timer-server" | "hpet-server" => PROCESS_PRIORITY_TIMER_SERVER,
        // IRQ resolution is a dependency of USB boot, not a low-priority app.
        // On one core, a READY AHCI probe can otherwise starve ACPI until the
        // higher-priority USB client's IRQ-resolution wait times out.
        "acpi-server" => PROCESS_PRIORITY_INPUT_SERVER,
        // Input pipeline must stay above the compositor and every input consumer.
        "input-server" | "ps2-server" | "usb-server" => PROCESS_PRIORITY_INPUT_SERVER,
        // GUI servers are above GUI clients, but below timer/input IRQ-facing services.
        "fb-server" | "honoka" => PROCESS_PRIORITY_GUI_SERVER,
        // Background servers stay above clients, but below the GUI critical path.
        "block-device-server" | "virtio-blk-server" | "ahci-server" => {
            PROCESS_PRIORITY_BACKGROUND_SERVER + 2
        }
        "driver-manager" => PROCESS_PRIORITY_TIMER_SERVER,
        "virtio-net" => PROCESS_PRIORITY_BACKGROUND_SERVER + 2,
        "ext2-server" => PROCESS_PRIORITY_BACKGROUND_SERVER + 1,
        "net-server" => PROCESS_PRIORITY_BACKGROUND_SERVER + 1,
        "rtc-server" => PROCESS_PRIORITY_BACKGROUND_SERVER + 1,
        "http-server" => PROCESS_PRIORITY_BACKGROUND_SERVER,
        "honoka-client" | "eg-test" | "image-viewer" | "performance-monitor" => {
            PROCESS_PRIORITY_INTERACTIVE_CLIENT
        }
        "shell" => PROCESS_PRIORITY_CLIENT,
        "cpp-hello" | "rust-hello" => PROCESS_PRIORITY_BACKGROUND_CLIENT,
        _ => PROCESS_PRIORITY_LOW,
    }
}
