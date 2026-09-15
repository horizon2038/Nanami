use super::*;
use input::Input;
use storage::Storage;
use xhci::Controller;

fn setup() -> (Storage, [Controller; 1]) {
    MANAGER.with(|manager| *manager.borrow_mut() = Manager::default());
    (Storage::new(25, false), [Controller::default()])
}
fn assert_manager(reports: &[usize], registrations: usize) {
    MANAGER.with(|manager| {
        let manager = manager.borrow();
        assert_eq!(manager.reports, reports);
        assert_eq!(manager.registrations, registrations);
    });
}
fn info(storage: &mut Storage, controllers: &mut [Controller]) -> Word {
    storage
        .request(
            ipc::ServiceRequest {
                identifier: 3,
                code: block::BLOCK_DEVICE_REQUEST_CONTROL,
                arg0: block::BLOCK_DEVICE_CONTROL_GET_INFO,
                arg1: 0,
                arg2: 0,
            },
            controllers,
            &mut Input,
        )
        .0
}

#[test]
fn late_storage_is_probed_and_published_once() {
    let (mut storage, mut controllers) = setup();
    storage.poll(&mut controllers, &mut Input, 24).unwrap();
    assert_manager(&[0], 0);
    controllers[0].attach();
    storage.poll(&mut controllers, &mut Input, 24).unwrap();
    assert_manager(&[0, 1], 1);
    assert_eq!(info(&mut storage, &mut controllers), OS_RESPONSE_OK);
    let commands = controllers[0].commands;
    for _ in 0..1000 {
        storage.poll(&mut controllers, &mut Input, 24).unwrap();
    }
    assert_manager(&[0, 1], 1);
    assert_eq!(controllers[0].commands, commands);
}

#[test]
fn waiting_without_topology_changes_does_not_rescan_or_send_ipc() {
    let (mut storage, mut controllers) = setup();
    for _ in 0..1000 {
        storage.poll(&mut controllers, &mut Input, 24).unwrap();
    }
    assert_eq!(controllers[0].scans, 1);
    assert_eq!(controllers[0].commands, 0);
    assert_manager(&[0], 0);
}

#[test]
fn initial_root_is_published_without_waiting_for_another_change() {
    let (mut storage, mut controllers) = setup();
    controllers[0].attach();
    storage.poll(&mut controllers, &mut Input, 24).unwrap();
    assert_manager(&[1], 1);
}

#[test]
fn changes_during_probe_are_enumerated_before_selection() {
    let (mut storage, mut controllers) = setup();
    controllers[0].attach();
    controllers[0].add_during_probe = true;
    storage.poll(&mut controllers, &mut Input, 24).unwrap();
    assert_manager(&[], 0); // First probe saw one root, but a second just arrived.
    storage.poll(&mut controllers, &mut Input, 24).unwrap();
    assert_manager(&[2], 0);
    controllers[0].detach();
    controllers[0].attach();
    storage.poll(&mut controllers, &mut Input, 24).unwrap();
    assert_manager(&[2], 0); // Ambiguity remains final even after unplugging one.
}

#[test]
fn detach_during_probe_discards_the_stale_candidate() {
    let (mut storage, mut controllers) = setup();
    controllers[0].attach();
    controllers[0].remove_during_probe = true;
    storage.poll(&mut controllers, &mut Input, 24).unwrap();
    assert_manager(&[], 0);
    storage.poll(&mut controllers, &mut Input, 24).unwrap();
    assert_manager(&[0], 0);
}

#[test]
fn mounted_root_is_not_reselected_after_detach() {
    let (mut storage, mut controllers) = setup();
    controllers[0].attach();
    storage.poll(&mut controllers, &mut Input, 24).unwrap();
    controllers[0].detach();
    assert_eq!(info(&mut storage, &mut controllers), OS_RESPONSE_FATAL);
    controllers[0].attach();
    storage.poll(&mut controllers, &mut Input, 24).unwrap();
    assert_eq!(info(&mut storage, &mut controllers), OS_RESPONSE_FATAL);
    assert_manager(&[1], 1);
}

#[test]
fn controller_failure_is_not_mistaken_for_an_empty_inventory() {
    let (mut storage, mut controllers) = setup();
    controllers[0].failed = true;
    storage.poll(&mut controllers, &mut Input, 24).unwrap();
    assert_manager(&[2], 0);
}

#[test]
fn partial_controller_initialization_cannot_publish_a_root() {
    let (_, mut controllers) = setup();
    let mut storage = Storage::new(25, true);
    controllers[0].attach();
    storage.poll(&mut controllers, &mut Input, 24).unwrap();
    assert_eq!(controllers[0].commands, 0);
    assert_manager(&[2], 0);
}

#[test]
fn roots_across_controllers_are_ambiguous() {
    let (mut storage, _) = setup();
    let mut controllers = [Controller::default(), Controller::default()];
    for controller in &mut controllers {
        controller.attach();
    }
    storage.poll(&mut controllers, &mut Input, 24).unwrap();
    assert_manager(&[2], 0);
}
