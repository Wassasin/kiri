#![no_std]

use core::{marker::PhantomData, ops::Add};

use csma_protocol::{Frame, FrameOwned, FrameRef, ReadResult, Reader};
use rand::{
    distributions::{uniform::SampleUniform, Uniform},
    prelude::Distribution,
    RngCore,
};

pub trait Transceiver {
    type Error;

    /// Whether the bus is currently idle. Some USART peripherals have a separate register to indicate this.
    fn bus_is_idle(&self) -> bool;

    /// Whether an error is detected on the bus, for example parity, framing or timing issues.
    ///
    /// This error must indicate that the sequence of bytes received is faulty.
    /// If the peripheral does not have this feature it is OK to always return `false`.
    fn bus_has_error(&self) -> bool;

    /// Write a byte on the bus.
    ///    /// Must yield `Ok` when completed.
    fn write(&self, byte: u8) -> nb::Result<(), Self::Error>;

    /// Read a byte from the bus, if available.
    fn read(&self) -> nb::Result<u8, Self::Error>;
}

pub trait Clock {
    type Instant: PartialEq + PartialOrd + Add<Self::Duration, Output = Self::Instant>;
    type Duration: PartialEq + SampleUniform;

    fn now(&self) -> Self::Instant;
}

pub trait Config<C: Clock> {
    const BUS_BIT_DURATION: C::Duration;
    const BUS_MAX_IDLE_DURATION: C::Duration;
}

#[derive(Debug)]
pub struct GreedyFrameInProgress {
    frame: Frame,
    ptr: usize,
}

impl GreedyFrameInProgress {
    pub fn first(&self) -> Option<u8> {
        return self.frame.0.get(self.ptr).map(|b| *b);
    }

    pub fn pop_first(&mut self) {
        self.ptr += 1;
    }

    pub fn reset(&mut self) {
        self.ptr = 0;
    }
}

impl GreedyFrameInProgress {
    pub fn new(frame: Frame) -> Self {
        Self { frame, ptr: 0 }
    }
}

/// Send your messages greedily. Do not listen on the line whether it is free.
pub struct GreedyStrategy<T: Transceiver> {
    transceiver: T,
    reader: Reader,
}

impl<T: Transceiver> GreedyStrategy<T> {
    pub fn new(transceiver: T) -> Self {
        Self {
            transceiver,
            reader: Reader::new(),
        }
    }

    pub fn send(&self, frame: &mut GreedyFrameInProgress) -> nb::Result<(), T::Error> {
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
            ReadResult::FrameOK(fr) => return Ok(fr),
            _ => return nb::Result::Err(nb::Error::WouldBlock),
        }
    }
}

enum CsmaStrategyState<C: Clock> {
    /// The bus is not idle, and before deciding to act we first must wait for a new frame.
    WaitForBusIdle,
    /// Bus is now idle, but needs to wait a bit before we can start chattering.
    BusIdleCooldown { ready_at: C::Instant },
    /// Clear any FIFO in queue. Peripheral should have processed all bytes by now.
    StartSend,
    /// We are sending a frame and hence the frame that we receive must correspond to our own frame.
    Sending,
    /// We have sent the last byte of the frame to the transceiver, and are awaiting it to come back
    /// through the transceiver.
    ///
    /// We will need to resend the frame if it does not end up back here.
    ConfirmingSendWithoutErrors,
}

/// Carrier Sense Multiple Access strategy implementation.
pub struct CsmaStrategy<T: Transceiver, C: Clock, R: RngCore, CONF: Config<C>> {
    transceiver: T,
    clock: C,
    rng: R,
    reader: Reader,
    state: CsmaStrategyState<C>,
    _conf: PhantomData<CONF>,
}

pub struct CsmaFrameInProgress {
    frame: Frame,
    send_ptr: usize,
    receive_ptr: usize,
}

impl CsmaFrameInProgress {
    pub fn reset(&mut self) {
        self.send_ptr = 0;
        self.receive_ptr = 0;
    }

    pub fn peek_for_send(&mut self) -> Option<u8> {
        self.frame.as_slice().get(self.send_ptr).map(|b| *b)
    }

    pub fn notify_send(&mut self) {
        self.send_ptr += 1;
    }

    pub fn feed_as_check(&mut self, b: u8) -> Result<bool, ()> {
        match self.frame.as_slice().get(self.receive_ptr) {
            Some(by) if *by == b => {
                self.receive_ptr += 1;
                Ok(self.receive_ptr == self.frame.as_slice().len())
            }
            _ => Err(()),
        }
    }
}

pub enum SendReceiveResult {
    SendComplete,
    Received(FrameOwned),
}

impl<T: Transceiver, C: Clock, R: RngCore, CONF: Config<C>> CsmaStrategy<T, C, R, CONF> {
    pub fn new(transceiver: T, clock: C, rng: R) -> Self {
        Self {
            transceiver,
            clock,
            rng,
            reader: Reader::new(),
            state: CsmaStrategyState::WaitForBusIdle,
            _conf: PhantomData::default(),
        }
    }

    /// Handle sending of bytes on bus, if the bus is clear.
    fn handle_send(&mut self, frame: &mut CsmaFrameInProgress) -> nb::Error<T::Error> {
        use CsmaStrategyState::*;
        match &self.state {
            WaitForBusIdle => {
                if self.transceiver.bus_is_idle() {
                    let distribution =
                        Uniform::new(CONF::BUS_BIT_DURATION, CONF::BUS_MAX_IDLE_DURATION);
                    let idle_duration = distribution.sample(&mut self.rng);
                    let ready_at = self.clock.now() + idle_duration;
                    self.state = BusIdleCooldown { ready_at };
                }
            }
            BusIdleCooldown { ready_at } => {
                if !self.transceiver.bus_is_idle() {
                    self.state = WaitForBusIdle;
                } else if self.clock.now() >= *ready_at {
                    self.state = StartSend;
                }
            }
            StartSend => {
                self.reader.clear();
                self.state = Sending;
            }
            Sending => {
                let b = match frame.peek_for_send() {
                    None => {
                        self.state = ConfirmingSendWithoutErrors;
                        return nb::Error::WouldBlock;
                    }
                    Some(b) => b,
                };

                if let nb::Result::Err(e) = self.transceiver.write(b) {
                    return e;
                }

                frame.notify_send();
                if frame.peek_for_send().is_none() {
                    self.state = ConfirmingSendWithoutErrors;
                }
            }
            ConfirmingSendWithoutErrors => (),
        }
        nb::Error::WouldBlock
    }

    /// Try to send a frame, but the strategy is open to receive a frame as well.
    ///
    /// Keep polling this function until `SendReceiveResult::SendComplete`.
    pub fn send_or_receive(
        &mut self,
        frame: &mut CsmaFrameInProgress,
    ) -> nb::Result<SendReceiveResult, T::Error> {
        use CsmaStrategyState::*;

        // Handle if there is an error on the bus.
        if self.transceiver.bus_has_error() {
            // Throw away any waiting bytes.
            let _ = self.transceiver.read();

            // Reset the current sending frame so that it is resent.
            frame.reset();

            // Forget the current incoming frame.
            self.reader.clear();

            // Wait for the error to clear and the bus to be reset again.
            self.state = WaitForBusIdle;
            return nb::Result::Err(nb::Error::WouldBlock);
        }

        // Handle incoming bytes during our sending process.
        if let Ok(b) = self.transceiver.read() {
            match &self.state {
                StartSend => {
                    // As long as we have bytes in the buffer when wanting to start to send, throw them away
                    // and reset the incoming frame.
                    self.reader.clear();
                    return nb::Result::Err(nb::Error::WouldBlock);
                }
                Sending | ConfirmingSendWithoutErrors => {
                    // Frame must correspond with the frame we are trying to send.
                    match frame.feed_as_check(b) {
                        Ok(true) => return Ok(SendReceiveResult::SendComplete),
                        Ok(false) => (), // Continue with sending.
                        Err(_) => {
                            // Mismatch between sending and loopback frames.

                            // Reset the current sending frame so that it is resent.
                            frame.reset();

                            // Forget the current incoming frame.
                            self.reader.clear();

                            // Wait for the error to clear and the bus to be reset again.
                            self.state = WaitForBusIdle;
                            return nb::Result::Err(nb::Error::WouldBlock);
                        }
                    }
                }
                _ => {
                    // The byte that we received is part of a valid frame.
                    if let ReadResult::FrameOK(incoming_frame) = self.reader.feed(b) {
                        // The frame that was finished should be the same as the one we are trying to send.
                        // If so, this indicates that the transceiver has succesfully sent our frame.

                        // The frame is not sent by us, and thus should be reported back to our caller.
                        return Ok(SendReceiveResult::Received(
                            incoming_frame.try_into().unwrap(),
                        ));
                    }
                }
            }
        }

        nb::Result::Err(self.handle_send(frame))
    }

    pub fn receive(&mut self) -> nb::Result<FrameRef<'_>, T::Error> {
        let b = self.transceiver.read()?;

        match self.reader.feed(b) {
            ReadResult::FrameOK(fr) => Ok(fr),
            _ => nb::Result::Err(nb::Error::WouldBlock),
        }
    }
}
