use std::{cell::Cell, rc::Rc};

use csma_csma::Transceiver;

use crate::clock::FakeClock;

#[derive(Debug, Clone, Copy)]
pub struct Fragment {
    contents: u8,
    error: bool,
}

pub struct SerialBus {
    fragment: Cell<Option<Fragment>>,
    clock: Rc<FakeClock>,
}

impl SerialBus {
    pub fn new(clock: Rc<FakeClock>) -> Self {
        Self {
            fragment: Cell::new(None),
            clock,
        }
    }

    pub fn write(&self, mut byte: u8) {
        // If two transceiver write at the same time, the message overlaps?
        let mut error = false;

        match self.fragment.get() {
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

        self.fragment.replace(Some(fragment));
    }

    pub fn is_idle(&self) -> bool {
        self.fragment.get().is_none()
    }

    pub fn is_error(&self) -> bool {
        match self.fragment.get() {
            Some(fragment) => fragment.error,
            None => false,
        }
    }

    pub fn read(&self) -> Option<u8> {
        self.fragment.get().map(|f| f.contents)
    }

    pub fn clear(&self) {
        self.fragment.replace(None);
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

    fn bus_has_error(&self) -> bool {
        self.bus.is_error()
    }

    fn write(&self, byte: u8) -> nb::Result<(), Self::Error> {
        self.bus.write(byte);
        Ok(())
    }

    fn read(&self) -> nb::Result<u8, Self::Error> {
        match self.bus.read() {
            Some(b) => Ok(b),
            None => nb::Result::Err(nb::Error::WouldBlock),
        }
    }
}
