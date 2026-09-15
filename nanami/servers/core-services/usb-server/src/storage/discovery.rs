use super::*;

pub(super) struct Discovery {
    manager: Word,
    pending: bool,
    failed: bool,
}

impl Discovery {
    pub(super) fn new(manager: Word, failed: bool) -> Self {
        Self {
            manager,
            pending: true,
            failed,
        }
    }
}

impl Storage {
    /// Probe on storage topology changes, not on every maintenance tick or I/O.
    /// Discovery ends permanently once a root is selected or selection fails.
    pub fn poll(
        &mut self,
        controllers: &mut [Controller],
        input: &mut Input,
        timer: Word,
    ) -> Result<(), RequestError> {
        let Some(discovery) = &mut self.discovery else {
            return Ok(());
        };
        for controller in controllers.iter_mut() {
            discovery.pending |= controller.take_storage_change();
        }
        if !core::mem::take(&mut discovery.pending) {
            return Ok(());
        }
        let probed = if discovery.failed {
            Err(RequestError::Protocol)
        } else {
            probe::probe(controllers, input, timer)
        };
        // SCSI waits can receive port changes. Do not publish a candidate from
        // an obsolete inventory; enumerate those changes before arbitration.
        for controller in controllers.iter_mut() {
            controller.poll(input);
            discovery.pending |= controller.take_storage_change();
        }
        if discovery.pending && !discovery.failed {
            return Ok(());
        }
        let roots = match &probed {
            Ok(None) => 0,
            Ok(Some(_)) => 1,
            Err(_) => 2,
        };
        let selected =
            nanami_services::device::select_storage_root_on_port(discovery.manager, roots);
        if matches!((&probed, &selected), (Ok(None), Ok(false))) {
            libnanami::print!("[usb-server] no root yet; watching storage connections\n");
            return Ok(());
        }
        // Keep this finished even if the mounted disk later detaches. Reusing
        // its block service for a replacement would redirect outstanding I/O.
        self.discovery = None;
        match (probed, selected) {
            (Ok(Some(root)), Ok(true)) => {
                self.root = Some(root);
                // usb-service and block-device deliberately alias the same port.
                nanami_services::registry::register_block_device()?;
                libnanami::print!("[usb-server] service registered: block-device\n");
            }
            (Err(error), _) | (_, Err(error)) => {
                libnanami::println!("[usb-server] root probe/selection failed: {}", error);
            }
            _ => {}
        }
        Ok(())
    }
}
