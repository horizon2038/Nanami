use a9n_abi::{CapabilityDescriptor, CapabilityError, KernelCallType, Sword};
use core::arch::asm;
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
    notification_call_with_identifier(
        notification_descriptor,
        a9n_abi::capability_call::notification_port::OperationType::Poll as Word,
    )
}

#[inline(always)]
fn notification_wait_zeroed(
    notification_descriptor: CapabilityDescriptor,
) -> Result<Word, CapabilityError> {
    notification_call_with_identifier(
        notification_descriptor,
        a9n_abi::capability_call::notification_port::OperationType::Wait as Word,
    )
}

#[inline(always)]
fn notification_call_with_identifier(
    notification_descriptor: CapabilityDescriptor,
    operation: Word,
) -> Result<Word, CapabilityError> {
    let mut a0 = notification_descriptor;
    let mut a1 = operation;
    let mut a2 = 0usize;

    unsafe {
        asm!(
            "syscall",
            in("rax") KernelCallType::CapabilityCall as Sword,
            inout("rdi") a0 => a0,
            inout("rsi") a1 => a1,
            inout("rdx") a2 => a2,
            out("rcx") _,
            out("r11") _,
            options(nostack),
            options(nomem),
        );
    }

    match a0 {
        0 => Err(match a1 {
            0 => CapabilityError::IllegalOperation,
            1 => CapabilityError::PermissionDenied,
            2 => CapabilityError::InvalidDescriptor,
            3 => CapabilityError::InvalidDepth,
            4 => CapabilityError::InvalidArgument,
            5 => CapabilityError::Fatal,
            _ => CapabilityError::DebugUnimplemented,
        }),
        _ => Ok(a2 as Word),
    }
}
