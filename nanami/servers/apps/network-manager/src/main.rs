#![no_std]
#![no_main]
#![feature(alloc_error_handler)]
extern crate alloc;
mod model;
mod view;
use alloc::format;
use model::{ip_text, Model};
use nanami_client::{net::*, timer::*};
use nanami_runtime::{ipc, RequestError, Word};
use nanami_ui::{
    window::{Event, Window},
    Font, Key, Keyboard, Rect,
};
nanami_runtime::nanami_entry!(main);
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    nanami_runtime::println!("[network-manager] panic: {}", info);
    let _ = nanami_runtime::request_exit();
    loop {
        core::hint::spin_loop();
    }
}
#[alloc_error_handler]
fn oom(_: core::alloc::Layout) -> ! {
    panic!("out of memory");
}
fn main() -> nanami_runtime::NanamiResult {
    ipc::init_ipc_tls()?;
    nanami_runtime::heap::init_heap(9 * 1024 * 1024)?;
    let font = Font::new();
    let mut window = Window::new("Network Manager", 180, 120, 740, 490)?;
    let result = run(&mut window, &font);
    window.close();
    result.map_err(Into::into)
}
fn status(port: Word, model: &mut Model) -> Result<bool, RequestError> {
    let (state, mode) = net_service_config_state(port)?;
    let (ip, _, _) = net_service_ipv4_config(port)?;
    let text = format!(
        "Active: {}  {}  {}",
        if mode == 0 { "DHCP" } else { "manual" },
        ip_text(ip),
        match state {
            IPV4_STATE_READY => "ready",
            IPV4_STATE_ACQUIRING => "acquiring lease...",
            IPV4_STATE_FAILED => "failed",
            _ => "unconfigured",
        }
    );
    let changed = text != model.active;
    model.active = text;
    Ok(changed)
}
fn action(port: Word, model: &mut Model, action: usize) {
    let result = (|| -> Result<(), RequestError> {
        if action == 0 {
            let settings = match model.settings() {
                Ok(settings) => settings,
                Err(message) => {
                    model.error = true;
                    model.message = message.into();
                    return Ok(());
                }
            };
            model.revision = net_service_save_settings(port, settings, model.revision)?;
            net_service_reload(port)?;
            model.message = "Settings saved and applied".into();
            model.edited = false;
        } else {
            if action == 1 {
                net_service_reload(port)?;
            }
            let (settings, revision) = net_service_settings(port)?;
            let active = core::mem::take(&mut model.active);
            *model = Model::new(settings, revision);
            model.active = active;
            model.message = if action == 1 {
                "Saved settings reloaded"
            } else {
                "Saved settings refreshed (not applied)"
            }
            .into();
        }
        model.error = false;
        Ok(())
    })();
    if let Err(error) = result {
        model.error = true;
        model.message = match error {
            RequestError::Status(NET_SERVICE_RESPONSE_CONFIG_CONFLICT) => {
                "Settings changed elsewhere; Refresh before retrying".into()
            }
            _ => format!("Operation failed: {}", error),
        };
    }
}
fn run(window: &mut Window, font: &Font) -> Result<(), RequestError> {
    nanami_client::registry::connect_network_service(24)?;
    nanami_client::registry::connect_timer_service(25)?;
    let port = ipc::process_slot_descriptor(24);
    let timer = ipc::process_slot_descriptor(25);
    let (settings, revision) = net_service_settings(port)?;
    let mut model = Model::new(settings, revision);
    nanami_runtime::println!("[network-manager] online");
    let mut keyboard = Keyboard::default();
    let mut pointer = (0, 0);
    timer_service_interval_on_notification_milliseconds(
        timer,
        500,
        nanami_runtime::PROCESS_SLOT_NOTIFICATION,
    )?;
    let result = (|| {
        let mut dirty = true;
        loop {
            for _ in 0..256 {
                let Some(event) = window.event()? else {
                    break;
                };
                match event {
                    Event::Close => return Ok(()),
                    Event::Key(code, pressed) => {
                        if let Some(key) = keyboard.event(code, pressed) {
                            match key {
                                Key::Control('q' | 'w') => return Ok(()),
                                Key::Control('s') => {
                                    action(port, &mut model, 0);
                                    dirty = true;
                                }
                                Key::F5 => {
                                    action(port, &mut model, 1);
                                    dirty = true;
                                }
                                _ => dirty |= model.key(key),
                            }
                        }
                    }
                    Event::Move(x, y) => pointer = (x, y),
                    Event::Button(1, true) => {
                        for (mode, x) in [(0, 20), (1, 200)] {
                            if Rect::new(x, 40, 165, 28).contains(pointer.0, pointer.1) {
                                model.mode = mode;
                                model.edited = true;
                                dirty = true;
                            }
                        }
                        for index in 0..4 {
                            if model.mode == 1
                                && view::field_rect(window.size().0, index)
                                    .contains(pointer.0, pointer.1)
                            {
                                model.focus = index;
                                model.fields[index].selected = true;
                                dirty = true;
                            }
                        }
                        for (id, x) in [(0, 20), (1, 175), (2, 330)] {
                            if Rect::new(x, 340, 140, 30).contains(pointer.0, pointer.1) {
                                action(port, &mut model, id);
                                dirty = true;
                            }
                        }
                    }
                    Event::Resize(_, _) => dirty = true,
                    _ => {}
                }
            }
            dirty |= status(port, &mut model)?;
            if dirty {
                window.paint(|canvas| view::draw(canvas, font, &model))?;
                dirty = false;
            }
            window.wait()?;
        }
    })();
    // Interval is owned by this connection and disappears when the process exits.
    result
}
