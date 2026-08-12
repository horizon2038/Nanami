use a9n_abi::{CapabilityDescriptor, CapabilityError};
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::{map_capability_error, RequestError, Word};

use super::tls::init_ipc_tls;

const SELF_PCB_DESCRIPTOR: CapabilityDescriptor = 0x0801_0000_0000_0000;
static BOUND_NOTIFICATION: AtomicUsize = AtomicUsize::new(0);
static PENDING_NOTIFICATION: AtomicUsize = AtomicUsize::new(0);

pub fn bind_current_thread_notification(
    notification_descriptor: CapabilityDescriptor,
) -> Result<(), RequestError> {
    init_ipc_tls()?;
    let config = a9n_abi::capability_call::process_control_block::ConfigurationInfo::new(
        false, // address_space
        false, // root_node
        false, // frame_ipc_buffer
        true,  // notification_port
        false, // ipc_port_resolver
        false, // instruction_pointer
        false, // stack_pointer
        false, // thread_local_base
        false, // priority
        false, // affinity
    );

    a9n_abi::arch::process_control_block::configure(
        SELF_PCB_DESCRIPTOR,
        config,
        0,
        0,
        0,
        notification_descriptor,
        0,
        0,
        0,
        0,
        0,
        0,
    )
    .map_err(map_capability_error)?;

    PENDING_NOTIFICATION.store(0, Ordering::Release);
    BOUND_NOTIFICATION.store(notification_descriptor, Ordering::Release);
    Ok(())
}

pub fn notification_wait(
    notification_descriptor: CapabilityDescriptor,
) -> Result<Word, RequestError> {
    if let Some(identifier) = take_interrupted_notification(notification_descriptor) {
        return Ok(identifier);
    }
    notification_wait_zeroed(notification_descriptor).map_err(map_capability_error)
}

pub fn notification_poll(
    notification_descriptor: CapabilityDescriptor,
) -> Result<Word, RequestError> {
    if let Some(identifier) = take_interrupted_notification(notification_descriptor) {
        return Ok(identifier);
    }
    notification_poll_zeroed(notification_descriptor).map_err(map_capability_error)
}

pub(crate) fn preserve_interrupted_notification(identifier: Word) {
    if identifier != 0 {
        PENDING_NOTIFICATION.fetch_or(identifier, Ordering::AcqRel);
    }
}

fn take_interrupted_notification(notification_descriptor: CapabilityDescriptor) -> Option<Word> {
    if BOUND_NOTIFICATION.load(Ordering::Acquire) != notification_descriptor {
        return None;
    }
    let identifier = PENDING_NOTIFICATION.swap(0, Ordering::AcqRel);
    (identifier != 0).then_some(identifier)
}

pub fn notification_notify(
    notification_descriptor: CapabilityDescriptor,
) -> Result<(), RequestError> {
    a9n_abi::arch::notification_port::notify(notification_descriptor).map_err(map_capability_error)
}

pub fn interrupt_ack(interrupt_descriptor: CapabilityDescriptor) -> Result<(), RequestError> {
    a9n_abi::arch::interrupt_port::ack(interrupt_descriptor).map_err(map_capability_error)
}

#[inline(always)]
fn notification_poll_zeroed(
    notification_descriptor: CapabilityDescriptor,
) -> Result<Word, CapabilityError> {
    a9n_abi::arch::notification_port::poll(notification_descriptor)
}

#[inline(always)]
fn notification_wait_zeroed(
    notification_descriptor: CapabilityDescriptor,
) -> Result<Word, CapabilityError> {
    a9n_abi::arch::notification_port::wait(notification_descriptor)
}
