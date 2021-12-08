#![no_std]

use core::marker::PhantomData;

use csma_protocol::{Frame, FrameRef, ReadResult, Reader, WriteResult, Writer};
use rand::RngCore;

pub trait Transceiver {
    type Error;

    fn write(&self, byte: u8) -> nb::Result<(), Self::Error>;
    fn read(&self) -> nb::Result<u8, Self::Error>;
}

pub trait Clock {
    fn now_ns(&self) -> u64;
}

pub trait Config {
    const BUS_FREQUENCY_HZ: u64;
}

// pub struct Csma<T: Transceiver, C: Clock, R: RngCore, CONF: Characteristics> {
//     transceiver: T,
//     clock: C,
//     rng: R,
//     _conf: PhantomData<CONF>,
// }

// impl<T: Transceiver, C: Clock, R: RngCore, CONF: Characteristics> Csma<T, C, R, CONF> {
//     pub fn new(transceiver: T, clock: C, rng: R) -> Self {
//         Self {
//             transceiver,
//             clock,
//             rng,
//             _conf: PhantomData::default(),
//         }
//     }
// }

/// Send your messages greedily. Do not listen on the line whether it is free.
pub struct GreedyStrategy<T: Transceiver, CONF: Config> {
    transceiver: T,
    reader: Reader,
    _conf: PhantomData<CONF>,
}

#[derive(Debug)]
pub struct FrameInProgress {
    frame: Frame,
    ptr: usize,
}

impl FrameInProgress {
    pub fn first(&self) -> Option<u8> {
        return self.frame.0.get(self.ptr).map(|b| *b);
    }

    pub fn pop_first(&mut self) {
        self.ptr += 1;
    }
}

impl FrameInProgress {
    pub fn new(frame: Frame) -> Self {
        Self { frame, ptr: 0 }
    }
}

impl<T: Transceiver, CONF: Config> GreedyStrategy<T, CONF> {
    pub fn new(transceiver: T) -> Self {
        Self {
            transceiver,
            reader: Reader::new(),
            _conf: PhantomData::default(),
        }
    }

    pub fn send(&self, frame: &mut FrameInProgress) -> nb::Result<(), T::Error> {
        // Note should be send_or_receive for CSMA.
        let b = match frame.first() {
            None => return nb::Result::Ok(()),
            Some(b) => b,
        };

        match self.transceiver.write(b) {
            Ok(()) => {
                frame.pop_first();
                match frame.first() {
                    Some(_) => nb::Result::Err(nb::Error::WouldBlock),
                    None => nb::Result::Ok(()),
                }
            }
            Err(e) => Err(e),
        }
    }

    pub fn receive(&mut self) -> nb::Result<FrameRef<'_>, T::Error> {
        let b = self.transceiver.read()?;

        match self.reader.feed(b) {
            ReadResult::FrameOK(fr) => Ok(fr),
            _ => nb::Result::Err(nb::Error::WouldBlock),
        }
    }
}

// Sense transceiver for activity
// if no activity start writing
// sense collisions
// abort and retry again
// randomness
