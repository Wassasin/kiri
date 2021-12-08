use std::{cell::Cell, rc::Rc};

use csma_csma::Transceiver;

use crate::clock::{FakeClock, Time};

#[derive(Debug, Clone, Copy)]
pub struct Fragment {
    contents: u8,
    since: Time,
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
        match self.fragment.get() {
            Some(ref old_fragment) => byte = byte | old_fragment.contents,
            None => (),
        }

        let fragment = Fragment {
            contents: byte,
            since: self.clock.now(),
        };

        self.fragment.replace(Some(fragment));
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
