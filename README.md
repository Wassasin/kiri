# Kiri
A communication protocol for embedded devices attached to a half-duplex RS485 bus.

This protocol implements a Data link layer and part of a Network layer, by providing framing, collision detection & recovery and limited addressing of nodes on the same RS485 physical layer. Within these frames you can put your own data.

Your Rust embedded device only needs:
* A RS485 transceiver or a RS232 transceiver with a RS485 transceiver behind it.
* A source of entropy.
* A stable clock.

## Physical layer
It is wise to use homogeneous RS485 transceivers, or at least transceivers with the same bus impedance. Depending on the length of the bus you might also want to reduce the communication frequency. Turning on parity bits is advised, but do adjust the timing parameters of the `kiri_csma::Configuration` accordingly to account for the extra bits used.

## Features
* Carrier-sense multiple access with collision detection, which is not suitable for radio-like applications but works well on a RS485 bus
* Explicit framing using COBS encoding
* CRC16

## Non-features
* Routing
* Acknowledgements