use std::{cell::RefCell, fmt::Debug, rc::Rc};

use kiri_csma::Transceiver;

#[derive(Debug, Clone, Copy)]
pub struct Fragment {
    contents: u8,
    error: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct SerialBusState {
    current: Option<Fragment>,
    next: Option<Fragment>,
}

pub struct SerialBus(RefCell<SerialBusState>);

impl SerialBus {
    pub fn new() -> Self {
        Self(RefCell::new(SerialBusState {
            current: None,
            next: None,
        }))
    }

    pub fn write(&self, mut byte: u8) {
        // If two transceiver write at the same time, the message overlaps?
        let mut error = false;

        let mut state = self.0.borrow_mut();

        match state.next {
            Some(ref old_fragment) => {
                byte = byte | old_fragment.contents;
                error = true;
            }
            None => (),
        }

        let fragment = Fragment {
            contents: byte,
            error,
        };

        state.next = Some(fragment);
    }

    pub fn is_idle(&self) -> bool {
        let state = self.0.borrow();
        state.current.is_none() && state.next.is_none()
    }

    pub fn is_error(&self) -> bool {
        let state = self.0.borrow();
        match state.current {
            Some(fragment) => fragment.error,
            None => false,
        }
    }

    pub fn read(&self) -> Option<u8> {
        let state = self.0.borrow();
        if let Some(current) = state.current {
            if !current.error {
                return Some(current.contents);
            }
        }
        None
    }

    pub fn iterate(&self) {
        let mut state = self.0.borrow_mut();

        state.current = state.next;
        state.next = None;
    }
}

pub struct SerialTransceiver {
    bus: Rc<SerialBus>,
}

impl SerialTransceiver {
    pub fn new(bus: Rc<SerialBus>) -> Self {
        Self { bus }
    }
}

impl Transceiver for SerialTransceiver {
    type Error = ();

    fn bus_is_idle(&self) -> bool {
        self.bus.is_idle()
    }

    fn write(&mut self, byte: u8) -> nb::Result<(), Self::Error> {
        self.bus.write(byte);
        Ok(())
    }

    fn read(&mut self) -> nb::Result<u8, kiri_csma::ReadError<Self::Error>> {
        if self.bus.is_error() {
            nb::Result::Err(nb::Error::Other(kiri_csma::ReadError::FrameError))
        } else {
            match self.bus.read() {
                Some(b) => Ok(b),
                None => nb::Result::Err(nb::Error::WouldBlock),
            }
        }
    }

    fn handle_interrupts(&self) {}
}

impl Debug for SerialTransceiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SerialTransceiver").finish()
    }
}
