#![no_std]

extern crate alloc;

use alloc::{format, vec::Vec};

use crc::{Crc, CRC_16_IBM_SDLC};
use deku::prelude::*;

const COBS_MARKER: u8 = 0;
const CHECKSUM: Crc<u16> = Crc::<u16>::new(&CRC_16_IBM_SDLC);
const CHECKSUM_LEN: usize = 2;

/// How long a message in the frame can be at most, chosen such that `MAX_FRAME_LEN` is at most `2048`.
pub const MAX_MESSAGE_LEN: usize = 2030;
/// How much bytes the header uses up. Do not forget the magic word.
pub const MAX_HEADER_LEN: usize = 6;
/// How much bytes the contents of a frame, without COBS encoding, taking up.
pub const MAX_NAKED_LEN: usize = MAX_MESSAGE_LEN + MAX_HEADER_LEN + CHECKSUM_LEN;
/// How large a frame can be, theoretically.
pub const MAX_FRAME_LEN: usize = cobs_max_encoding_length(MAX_NAKED_LEN) + 1;

/// How much bytes cobs will use at most given a specific source length.
const fn cobs_max_encoding_length(source_len: usize) -> usize {
    source_len + (source_len / 254) + if source_len % 254 > 0 { 1 } else { 0 }
}

#[derive(Debug, PartialEq, DekuRead, DekuWrite, Clone, Copy)]
#[deku(endian = "big")]
pub struct Address(#[deku(bits = 10)] pub u16);

#[derive(Debug, PartialEq, DekuRead, DekuWrite)]
#[deku(magic = b"lg")]
pub struct Header {
    pub src: Address,
    pub dst: Address,
    #[deku(endian = "big", bits = "11")]
    pub len: u16,
    #[deku(bits = "1")]
    _misc: bool,
}

/// A reference to a decoded frame, owned by the Reader.
///
/// Will clean the frame up once the reference is no longer used.
/// Locks the reader from being fed as long as the reference is intact.
#[derive(Debug, PartialEq)]
pub struct FrameRef<'a> {
    pub header: Header,
    pub contents: &'a [u8],
}

/// Owned variant of a frame.
///
/// **TODO**: remove this type as it should be unnecessary.
pub struct FrameOwned {
    pub header: Header,
    pub contents: heapless::Vec<u8, MAX_MESSAGE_LEN>,
}

impl<'a> TryInto<FrameOwned> for FrameRef<'a> {
    type Error = ();

    fn try_into(self) -> Result<FrameOwned, Self::Error> {
        let contents = heapless::Vec::from_slice(self.contents)?;
        Ok(FrameOwned {
            header: self.header,
            contents,
        })
    }
}

#[derive(Debug, PartialEq)]
pub enum ReadResult<'a> {
    /// The reader has not yet consumed enough bytes.
    NotYet,
    /// We have run out of buffer.
    Overflow,
    /// Frame is invalid because it is not encoded with COBS correctly.
    FrameErrorCobs,
    /// Frame is invalid because the header is broken.
    FrameErrorHeader,
    /// Frame is invalid because the content length does not correspond to the length in the header.
    FrameErrorSize,
    /// Frame is invalid because the content checksum is incorrect.
    FrameErrorChecksum,
    /// Frame is OK, here is it.
    FrameOK(FrameRef<'a>),
}

impl<'a> ReadResult<'a> {
    pub fn is_error(&self) -> bool {
        match self {
            ReadResult::NotYet => false,
            ReadResult::FrameOK(_) => false,

            ReadResult::Overflow
            | ReadResult::FrameErrorCobs
            | ReadResult::FrameErrorHeader
            | ReadResult::FrameErrorSize
            | ReadResult::FrameErrorChecksum => true,
        }
    }
}

/// A reader for the protocol.
///
/// We use a separate `ptr` field contrary to a `heapless::Vec` due to lifetimes.
pub struct Reader {
    buf: [u8; MAX_FRAME_LEN],
    ptr: usize,
}

impl Reader {
    pub fn new() -> Self {
        Reader {
            buf: [0u8; MAX_FRAME_LEN],
            ptr: 0,
        }
    }

    pub fn clear(&mut self) {
        self.ptr = 0;
    }

    /// Feed a new byte to the reader, and it might result in a correct frame.
    ///
    /// Do not forget to clear the reader after an error.
    pub fn feed(&mut self, byte: u8) -> ReadResult {
        let old_ptr = self.ptr;
        let new_ptr = (self.ptr + 1).min(self.buf.len());
        let overflown = old_ptr == new_ptr;

        if overflown {
            return ReadResult::Overflow;
        }

        self.buf[self.ptr] = byte;
        self.ptr = new_ptr;

        // COBS marker detected
        if byte == COBS_MARKER {
            // Clear frame so that the reader is usable again at error or when FrameRef is dropped.
            self.clear();

            let msg_buf = &mut self.buf[0..old_ptr];
            let msg_buf = match cobs::decode_in_place(msg_buf) {
                Ok(len) => &mut msg_buf[0..len],
                Err(()) => return ReadResult::FrameErrorCobs,
            };

            // We want at least the checksum tail in there.
            if msg_buf.len() < CHECKSUM_LEN {
                return ReadResult::FrameErrorSize;
            }

            let (msg_buf, checksum_buf) = msg_buf.split_at(msg_buf.len() - CHECKSUM_LEN);
            let checksum_at_end = u16::from_be_bytes(checksum_buf.try_into().unwrap());
            let checksum_of_msg = CHECKSUM.checksum(msg_buf);

            if checksum_at_end != checksum_of_msg {
                return ReadResult::FrameErrorChecksum;
            }

            let (body_buf, header) = match Header::from_bytes((msg_buf, 0)) {
                // Nice body and header has no bits remaining.
                Ok(((body_buf, 0), header)) => (body_buf, header),
                // Nice body but we have some weird bits left.
                Ok(_) => {
                    unreachable!("Header should be rounded bytes without left-over bits")
                }
                Err(_) => return ReadResult::FrameErrorHeader,
            };

            if body_buf.len() != header.len as usize {
                return ReadResult::FrameErrorSize;
            }

            // Reader can not be fed as long as FrameRef is in use.
            ReadResult::FrameOK(FrameRef {
                header,
                contents: body_buf,
            })
        } else {
            ReadResult::NotYet
        }
    }
}

#[derive(Debug)]
pub struct Frame(pub heapless::Vec<u8, { MAX_FRAME_LEN }>);

impl Frame {
    pub fn as_slice(&self) -> &[u8] {
        self.0.as_slice()
    }
}

#[derive(Debug)]
pub enum WriteResult {
    /// Tried to write a message that will not fit within a frame.
    TooLong,
    /// Tried to encode an invalid header.
    FrameErrorHeader,
    /// Frame finished, here is it.
    FrameOK(Frame),
}

pub struct Writer;

impl Writer {
    pub fn package(src: Address, dst: Address, contents: &[u8]) -> WriteResult {
        let len: u16 = match contents.len().try_into() {
            Ok(len) => len,
            Err(_) => return WriteResult::TooLong,
        };

        let header = Header {
            src,
            dst,
            len,
            _misc: false,
        };

        let mut buf = heapless::Vec::<u8, { MAX_FRAME_LEN }>::new();
        buf.resize_default(MAX_FRAME_LEN).unwrap();

        let mut cobs = cobs::CobsEncoder::new(buf.as_mut());
        let mut checksum_digest = CHECKSUM.digest();

        match header.to_bytes() {
            Ok(header_buf) => {
                checksum_digest.update(&header_buf);
                match cobs.push(&header_buf) {
                    Ok(()) => (),
                    Err(_) => return WriteResult::TooLong, // Should never happen.
                }
            }
            Err(_) => return WriteResult::FrameErrorHeader,
        };

        checksum_digest.update(contents);
        match cobs.push(contents) {
            Ok(()) => (),
            Err(_) => return WriteResult::TooLong, // Can definitely happen.
        }

        let crc = checksum_digest.finalize();
        match cobs.push(&crc.to_be_bytes()) {
            Ok(()) => (),
            Err(_) => return WriteResult::TooLong, // Can definitely happen.
        }

        match cobs.finalize() {
            Ok(len) => {
                if len < buf.len() {
                    // Add COBS sentinel marker.
                    buf[len] = COBS_MARKER;
                    buf.resize_default(len + 1).unwrap();
                    WriteResult::FrameOK(Frame(buf))
                } else {
                    WriteResult::TooLong
                }
            }
            Err(_) => WriteResult::TooLong,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::*;
    use alloc::vec;

    const MSG: &[u8] = b"\0loremipsum\0";
    const ADDR_A: Address = Address(13);
    const ADDR_B: Address = Address(169);

    fn fill_frame(result: &mut [u8]) -> &mut [u8] {
        let frame = match Writer::package(ADDR_A, ADDR_B, MSG) {
            WriteResult::FrameOK(frame) => frame,
            e => panic!("Invalid result {:?}", e),
        };

        let result = &mut result[0..frame.as_slice().len()];
        result.copy_from_slice(frame.as_slice());
        result
    }

    #[test]
    fn unchanged_header() {
        let header = Header {
            src: Address(13),
            dst: Address(1023),
            len: 1337,
            _misc: false,
        };

        assert_eq!(vec![108, 103, 3, 127, 250, 114], header.to_bytes().unwrap());
        assert_eq!(
            Header::from_bytes((&header.to_bytes().unwrap(), 0)).unwrap(),
            (([0u8; 0].as_slice(), 0), header)
        );
    }

    #[test]
    fn writer_reader_ok() {
        let frame = &mut [0u8; 4096];
        let frame = fill_frame(frame);

        let (frame_last, frame_begin) = frame.split_last().unwrap();
        assert_eq!(*frame_last, COBS_MARKER);

        let mut reader = Reader::new();
        for b in frame_begin {
            let feed_result = reader.feed(*b);
            assert_eq!(feed_result, ReadResult::NotYet);
        }

        let frame = match reader.feed(*frame_last) {
            ReadResult::FrameOK(frame) => frame,
            e => panic!("Invalid result {:?}", e),
        };

        assert_eq!(frame.header.src, ADDR_A);
        assert_eq!(frame.header.dst, ADDR_B);
        assert_eq!(frame.contents, MSG);
    }

    #[test]
    fn writer_reader_noise() {
        let frame = &mut [0u8; MAX_FRAME_LEN];
        let len = fill_frame(frame).len();

        for i in 0..(len - 1) {
            fill_frame(frame);

            // Add some noise
            let (x, _) = frame[i].overflowing_add(1);
            frame[i] = x;

            let (frame_last, frame_begin) = frame.split_last().unwrap();
            assert_eq!(*frame_last, COBS_MARKER);

            let mut reader = Reader::new();
            for (j, b) in frame_begin.iter().enumerate() {
                match reader.feed(*b) {
                    ReadResult::NotYet => (),
                    ReadResult::FrameOK(_) => {
                        panic!("Frame can not be OK midframe @ {} with error @ {}", j, i)
                    }
                    ReadResult::Overflow
                    | ReadResult::FrameErrorCobs
                    | ReadResult::FrameErrorHeader
                    | ReadResult::FrameErrorSize
                    | ReadResult::FrameErrorChecksum => continue, // Test OK
                }
            }

            match reader.feed(*frame_last) {
                ReadResult::FrameOK(_) => panic!("Frame can not be OK with error @ {}", i),
                _ => continue, // Test OK
            };
        }
    }
}
