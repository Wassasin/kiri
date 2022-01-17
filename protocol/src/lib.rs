#![no_std]

use core::fmt::Debug;
use packed_struct::{prelude::*, types::Integer};

use crc::{Crc, CRC_16_IBM_SDLC};

const COBS_MARKER: u8 = 0;
const CHECKSUM: Crc<u16> = Crc::<u16>::new(&CRC_16_IBM_SDLC);
const CHECKSUM_LEN: usize = 2;

const MAGIC_LEN: usize = 2;
const MAGIC_WORD: &[u8; 2] = b"kI";

/// How much bytes the header uses up.
pub const HEADER_LEN: usize = 4;

/// How long a message in the frame can be at most, chosen such that `MAX_FRAME_LEN` is at most `1024`.
pub const MAX_MESSAGE_LEN: usize = 1006;

/// How much bytes the contents of a frame, without COBS encoding, is taking up at most.
pub const MAX_NAKED_LEN: usize = MAGIC_LEN + HEADER_LEN + MAX_MESSAGE_LEN + CHECKSUM_LEN;

/// How much bytes the contents of a frame, without COBS encoding, is taking up at least.
pub const MIN_NAKED_LEN: usize = MAGIC_LEN + HEADER_LEN + CHECKSUM_LEN;

/// How large a frame can be, theoretically.
pub const MAX_FRAME_LEN: usize = cobs_max_encoding_length(MAX_NAKED_LEN) + 1;

/// How much bytes cobs will use at most given a specific source length.
const fn cobs_max_encoding_length(source_len: usize) -> usize {
    source_len + (source_len / 254) + if source_len % 254 > 0 { 1 } else { 0 }
}

#[derive(PackedStruct, Debug, PartialEq, Clone, Copy)]
#[packed_struct(bit_numbering = "msb0", endian = "msb")]
pub struct Address {
    #[packed_field(bits = "6..16")]
    inner: Integer<u16, packed_bits::Bits<10>>,
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub struct AddressTooLargeError;

const ADDRESS_UNICAST: u16 = 0x3FF;

impl Address {
    pub fn new(addr: u16) -> Result<Self, AddressTooLargeError> {
        let inner = convert_primitive(addr).map_err(|_| AddressTooLargeError)?;
        Ok(Self { inner })
    }

    pub fn unicast() -> Address {
        Self::new(ADDRESS_UNICAST).unwrap()
    }

    pub fn is_unicast(&self) -> bool {
        self == &Self::unicast()
    }

    pub fn to_primitive(&self) -> u16 {
        self.inner.to_primitive()
    }
}

#[derive(PackedStruct, Debug, PartialEq, Clone)]
#[packed_struct(bit_numbering = "msb0", endian = "msb", size_bytes = 4)]
pub struct Header {
    #[packed_field(bits = "0..10")]
    pub address_src: Address,
    #[packed_field(bits = "10..20")]
    pub address_dst: Address,
    #[packed_field(bits = "20..30")]
    pub len: Integer<u16, packed_bits::Bits<10>>,
    // #[packed_field(bits = "30..33")]
    // _seq: Integer<u8, packed_bits::Bits<3>>,
    // #[packed_field(bits = "33..36")]
    // _ack: Integer<u8, packed_bits::Bits<3>>,
    #[packed_field(bits = "30..32")]
    _reserved: Integer<u8, packed_bits::Bits<2>>,
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

impl<'a> From<&'a FrameOwned> for FrameRef<'a> {
    fn from(val: &'a FrameOwned) -> Self {
        FrameRef {
            header: val.header.clone(),
            contents: val.contents.as_slice(),
        }
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
    /// Frame is invalid because the magic word is not correct.
    FrameErrorMagic,
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
            | ReadResult::FrameErrorMagic
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

            let buf = &mut self.buf[0..old_ptr];
            let buf = match cobs::decode_in_place(buf) {
                Ok(len) => &mut buf[0..len],
                Err(()) => return ReadResult::FrameErrorCobs,
            };

            if buf.len() < MIN_NAKED_LEN {
                return ReadResult::FrameErrorSize;
            }

            let (buf, checksum_buf) = buf.split_at(buf.len() - CHECKSUM_LEN);
            let checksum_at_end = u16::from_be_bytes(checksum_buf.try_into().unwrap());
            let checksum_of_msg = CHECKSUM.checksum(buf);

            if checksum_at_end != checksum_of_msg {
                return ReadResult::FrameErrorChecksum;
            }

            let (magic_buf, buf) = buf.split_at(MAGIC_LEN);
            let (header_buf, content_buf) = buf.split_at(HEADER_LEN);

            if magic_buf != MAGIC_WORD {
                return ReadResult::FrameErrorHeader;
            }

            let header_buf: &[u8; HEADER_LEN] = header_buf.try_into().unwrap();

            let header = match Header::unpack(header_buf) {
                Ok(header) => header,
                Err(_) => return ReadResult::FrameErrorHeader,
            };

            if content_buf.len() != header.len.to_primitive() as usize {
                return ReadResult::FrameErrorSize;
            }

            // Reader can not be fed as long as FrameRef is in use.
            ReadResult::FrameOK(FrameRef {
                header,
                contents: content_buf,
            })
        } else {
            ReadResult::NotYet
        }
    }
}

impl Default for Reader {
    fn default() -> Self {
        Self::new()
    }
}

impl Debug for Reader {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.buf[0..self.ptr].fmt(f)
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
#[allow(clippy::large_enum_variant)]
pub enum WriteError {
    /// Tried to write a message that will not fit within a frame.
    TooLong,
    /// Tried to encode an invalid header.
    FrameErrorHeader,
}

pub struct Writer;

impl Writer {
    pub fn package(src: Address, dst: Address, contents: &[u8]) -> Result<Frame, WriteError> {
        use WriteError::*;

        let len = match contents
            .len()
            .try_into()
            .map_err(|_| ())
            .and_then(convert_primitive)
        {
            Ok(len) => len,
            Err(_) => return Err(TooLong),
        };

        let header = Header {
            address_src: src,
            address_dst: dst,
            len,
            _reserved: Integer::from_primitive(0),
        };

        let mut buf = heapless::Vec::<u8, { MAX_FRAME_LEN }>::new();
        buf.resize_default(MAX_FRAME_LEN).unwrap();

        let mut cobs = cobs::CobsEncoder::new(buf.as_mut());
        let mut checksum_digest = CHECKSUM.digest();

        let header_buf = match header.pack() {
            Ok(header_buf) => header_buf,
            Err(_) => return Err(FrameErrorHeader),
        };

        checksum_digest.update(MAGIC_WORD.as_slice());
        cobs.push(MAGIC_WORD.as_slice()).unwrap(); // Unwrap: can never happen due to buffer size.

        checksum_digest.update(&header_buf);
        cobs.push(&header_buf).unwrap(); // Unwrap: can never happen due to buffer size.

        checksum_digest.update(contents);
        match cobs.push(contents) {
            Ok(()) => (),
            Err(_) => return Err(TooLong), // Can definitely happen.
        }

        let crc = checksum_digest.finalize();
        match cobs.push(&crc.to_be_bytes()) {
            Ok(()) => (),
            Err(_) => return Err(TooLong), // Can definitely happen.
        }

        match cobs.finalize() {
            Ok(len) => {
                if len < buf.len() {
                    // Add COBS sentinel marker.
                    buf[len] = COBS_MARKER;
                    buf.resize_default(len + 1).unwrap();
                    Ok(Frame(buf))
                } else {
                    Err(TooLong)
                }
            }
            Err(_) => Err(TooLong),
        }
    }
}

/// Convert a primitive integer to a bit constrained version, checking whether the number fits.
fn convert_primitive<T, U, const B: usize>(i: T) -> Result<U, ()>
where
    U: SizedInteger<T, packed_bits::Bits<B>>,
    packed_bits::Bits<B>: packed_bits::NumberOfBits,
    T: PartialEq + Clone,
{
    let res = U::from_primitive(i.clone());
    if res.to_primitive() == i {
        Ok(res)
    } else {
        Err(())
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;

    use crate::*;
    use alloc::vec;

    const MSG: &[u8] = b"\0loremipsum\0";
    const ADDR_A: u16 = 13;
    const ADDR_B: u16 = 169;

    fn fill_frame(result: &mut [u8]) -> &mut [u8] {
        let frame = match Writer::package(
            Address::new(ADDR_A).unwrap(),
            Address::new(ADDR_B).unwrap(),
            MSG,
        ) {
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
            address_src: Address::new(13).unwrap(),
            address_dst: Address::new(1023).unwrap(),
            len: Integer::from_primitive(800),
            _reserved: Integer::from_primitive(0),
        };

        assert_eq!(vec![3, 127, 252, 128], header.pack().unwrap());
        assert_eq!(Header::unpack(&header.pack().unwrap()).unwrap(), header);
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

        assert_eq!(frame.header.address_src, Address::new(ADDR_A).unwrap());
        assert_eq!(frame.header.address_dst, Address::new(ADDR_B).unwrap());
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
                    | ReadResult::FrameErrorMagic
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
