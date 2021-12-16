use log;
use rand::prelude::ThreadRng;
use serde_derive::{Deserialize, Serialize};
use std::{collections::HashSet, rc::Rc};

use clock::{FakeClock, FakeDuration};
use csma_csma::{Clock, CsmaFrameInProgress, CsmaStrategy, SendReceiveResult};
use csma_protocol::{Address, Frame, FrameRef, Writer};
use simulation::{SerialBus, SerialTransceiver};

mod clock;
mod simulation;

#[derive(Debug)]
pub struct BusConf;

impl csma_csma::Config<FakeClock> for BusConf {
    const BUS_MIN_IDLE_DURATION: <FakeClock as Clock>::Duration = FakeDuration(1);
    const BUS_MAX_IDLE_DURATION: <FakeClock as Clock>::Duration = FakeDuration(32);
}

pub struct Mailbox {
    messages_per_party: usize,
    send_progress: Vec<usize>,
    receive_progress: Vec<HashSet<usize>>,
}

impl Mailbox {
    pub fn new(messages_per_party: usize, parties: usize) -> Self {
        Self {
            messages_per_party,
            send_progress: Vec::from_iter((0..parties).map(|_| 0)),
            receive_progress: Vec::from_iter((0..parties).map(|_| HashSet::default())),
        }
    }

    /// Fetch a new message to send.
    pub fn fetch(&mut self, src: Address) -> Option<Frame> {
        // TODO maybe wait for messages to be generated.
        let addr = src.0 as usize;
        let parties = self.send_progress.len();
        let progress = &mut self.send_progress[addr];

        if *progress < self.messages_per_party {
            let mut dst = *progress % (parties - 1);
            if dst >= addr {
                dst += 1;
            }
            let dst = Address(dst as u16);
            let message = Message {
                src: src.0,
                dst: dst.0,
                identifier: *progress,
            };

            let frame = match Writer::package(src, dst, &message.to_bytes()) {
                csma_protocol::WriteResult::FrameOK(frame) => frame,
                _ => panic!("Writer failed to pack reasonable message"),
            };

            log::info!("Sending {} -> {}: {}", src.0, dst.0, progress);

            *progress += 1;

            Some(frame)
        } else {
            None
        }
    }

    pub fn deliver(&mut self, frame: FrameRef) {
        let message = Message::from_bytes(frame.contents).unwrap();
        assert_eq!(message.src, frame.header.src.0);
        assert_eq!(message.dst, frame.header.dst.0);
        self.receive_progress[message.src as usize].insert(message.identifier);

        log::info!(
            "Received {} -> {}: {}",
            message.src,
            message.dst,
            message.identifier
        );
    }

    pub fn report(&self) {
        log::info!(
            "{:?} {:?}",
            self.send_progress,
            Vec::from_iter(self.receive_progress.iter().map(|set| set.len()))
        );
        log::info!(
            "{}% received",
            self.receive_progress
                .iter()
                .map(|set| set.len())
                .sum::<usize>() as f64
                / self.messages_per_party as f64
                / self.receive_progress.len() as f64
                * 100.
        );
    }
}

pub struct Party<'a> {
    address: Address,
    strategy: CsmaStrategy<'a, SerialTransceiver, FakeClock, ThreadRng, BusConf>,
    current_frame: Option<CsmaFrameInProgress>,
}

impl<'a> Party<'a> {
    pub fn new(
        address: Address,
        strategy: CsmaStrategy<'a, SerialTransceiver, FakeClock, ThreadRng, BusConf>,
    ) -> Self {
        Self {
            address,
            strategy,
            current_frame: None,
        }
    }

    pub fn simulate(&mut self, mailbox: &mut Mailbox) {
        if self.current_frame.is_none() {
            self.current_frame = mailbox
                .fetch(self.address)
                .map(|frame| CsmaFrameInProgress::new(frame));
        }

        if let Some(frame) = self.current_frame.as_mut() {
            log::trace!("{:?} (S/R) {:?} {:?}", self.address, self.strategy, frame);
            match self.strategy.send_or_receive(frame) {
                Ok(SendReceiveResult::Received(incoming_frame)) => {
                    if incoming_frame.header.dst == self.address {
                        mailbox.deliver((&incoming_frame).into())
                    }
                }
                Ok(SendReceiveResult::SendComplete) => self.current_frame = None,
                Err(nb::Error::WouldBlock) => (),
                Err(nb::Error::Other(e)) => panic!("Error: {:?}", e),
            }
        } else {
            log::trace!("{:?} (R) {:?}", self.address, self.strategy);
            match self.strategy.receive() {
                Ok(frame) => {
                    if frame.header.dst == self.address {
                        mailbox.deliver(frame)
                    }
                }
                Err(nb::Error::WouldBlock) => (),
                Err(nb::Error::Other(e)) => panic!("Error: {:?}", e),
            }
        }
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    src: u16,
    dst: u16,
    identifier: usize,
}

impl Message {
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap()
    }

    pub fn from_bytes(buf: &[u8]) -> Result<Self, ()> {
        serde_json::from_slice(buf).map_err(|_| ())
    }
}

fn main() {
    pretty_env_logger::init();

    let clock = Rc::new(FakeClock::new());
    let bus = Rc::new(SerialBus::new());

    let message_count = 1000;
    let party_count = 10;

    let mut mailbox = Mailbox::new(message_count, party_count);

    let mut parties = Vec::new();
    parties.reserve(party_count);

    for i in 0..party_count {
        let address = Address(i as u16);
        let transceiver = SerialTransceiver::new(bus.clone());
        let strategy =
            CsmaStrategy::<_, _, _, BusConf>::new(transceiver, &clock, rand::thread_rng());
        parties.push(Party::new(address, strategy));
    }

    let len = 20000000;

    for _i in 0..len {
        bus.iterate();

        for p in parties.iter_mut() {
            p.simulate(&mut mailbox);
        }

        let states = Vec::from_iter(parties.iter().map(|p| &p.strategy.state));
        log::trace!("{:?} {:?} {:?}", clock.now(), bus.read(), states);
        clock.increase(1);
    }

    mailbox.report();
}
