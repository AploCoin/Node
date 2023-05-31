use crate::errors::{models_errors::AddressError, *};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

// #[repr(u8)]
// #[derive(Clone)]
// pub enum NodeCommand {
//     Stop,
//     Subscribed,
// }

pub mod packet_models {
    use super::*;

    #[repr(u8)]
    #[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize, Serialize)]
    pub enum ErrorCode {
        ParseError = 1,
        BadAddress,
        BadBlockchainAddress,
        UnexpectedInternalError,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
    pub enum Packet {
        Request(Request),
        Response(Response),
        Error(ErrorR),
    }

    #[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
    #[serde(tag = "q")]
    pub enum Request {
        GetNodes(GetNodesRequest),
        GetAmount(GetAmountRequest),
        GetTransaction(GetTransactionRequest),
        Announce(AnnounceRequest),
        Ping(PingRequest),
        GetBlockByHash(GetBlockByHashRequest),
        GetBlockByHeight(GetBlockByHeightRequest),
        GetBlocksByHeights(GetBlocksByHeightsRequest),
        NewTransaction(NewTransactionRequest),
        SubmitPow(SubmitPow),
        NewBlock(NewBlockRequest),
    }

    #[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
    pub struct NewBlockRequest {
        pub id: u64,
        pub dump: Vec<u8>,
        pub transactions: Vec<u8>,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
    pub struct SubmitPow {
        pub id: u64,
        pub pow: Vec<u8>,
        pub address: Vec<u8>,
        pub timestamp: u64,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
    pub struct GetNodesRequest {
        pub id: u64,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
    pub struct GetAmountRequest {
        pub id: u64,
        pub address: Vec<u8>,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
    pub struct GetTransactionRequest {
        pub id: u64,
        pub hash: [u8; 32],
    }

    #[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
    pub struct GetBlockByHashRequest {
        pub id: u64,
        pub hash: [u8; 32],
    }

    #[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
    pub struct NewTransactionRequest {
        pub id: u64,
        pub transaction: Vec<u8>,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
    pub struct GetBlockByHeightRequest {
        pub id: u64,
        pub height: u64,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
    pub struct GetBlocksByHeightsRequest {
        pub id: u64,
        pub start: u64,
        pub amount: u64,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
    pub struct AnnounceRequest {
        pub id: u64,
        pub addr: Vec<u8>,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
    pub struct PingRequest {
        pub id: u64,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
    #[serde(tag = "r")]
    pub enum Response {
        Ok(OkResponse),
        GetNodes(GetNodesReponse),
        GetAmount(GetAmountResponse),
        GetTransaction(GetTransactionResponse),
        Ping(PingResponse),
        GetBlock(GetBlockResponse),
        GetBlocks(GetBlocksResponse),
        SubmitPow(SubmitPowResponse),
    }

    #[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
    pub struct SubmitPowResponse {
        pub id: u64,
        pub accepted: bool,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
    pub struct OkResponse {
        pub id: u64,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
    pub struct ErrorR {
        pub code: ErrorCode,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
    pub struct GetNodesReponse {
        pub id: u64,
        pub ipv4: Option<Vec<u8>>,
        pub ipv6: Option<Vec<u8>>,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
    pub struct GetAmountResponse {
        pub id: u64,
        pub amount: Vec<u8>,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
    pub struct GetBlockResponse {
        pub id: u64,
        pub dump: Option<Vec<u8>>,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
    pub struct GetBlocksResponse {
        pub id: u64,
        pub blocks: Vec<Vec<u8>>,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
    pub struct GetTransactionResponse {
        pub id: u64,
        pub transaction: Option<Vec<u8>>,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
    pub struct PingResponse {
        pub id: u64,
    }

    #[cfg(test)]
    mod packet_tests {
        use super::*;
        use rmp_serde::{Deserializer, Serializer};
        use std::io::Cursor;

        #[test]
        fn test_error() {
            let mut buf: Vec<u8> = Vec::new();

            let obj = Packet::Error(ErrorR {
                code: ErrorCode::ParseError,
            });

            obj.serialize(&mut Serializer::new(&mut buf)).unwrap();

            let deserialized =
                Packet::deserialize(&mut Deserializer::new(Cursor::new(buf))).unwrap();

            assert_eq!(obj, deserialized);
        }

        #[test]
        fn test_request() {
            let mut buf: Vec<u8> = Vec::new();

            let mut addr: Vec<u8> = Vec::with_capacity(6);
            addr.push(127);
            addr.push(0);
            addr.push(0);
            addr.push(1);
            addr.push(0);
            addr.push(255);

            let obj = Packet::Request(Request::Announce(AnnounceRequest { id: 20, addr }));

            obj.serialize(&mut Serializer::new(&mut buf)).unwrap();

            let deserialized =
                Packet::deserialize(&mut Deserializer::new(Cursor::new(buf))).unwrap();

            assert_eq!(obj, deserialized);
        }
    }
}

pub mod peers_dump {
    use super::*;

    #[derive(Debug, Deserialize, Serialize)]
    pub struct Peers {
        pub ipv4: Option<Vec<u8>>,
        pub ipv6: Option<Vec<u8>>,
    }
}

#[warn(dead_code)]
pub fn addr2bin(addr: &SocketAddr) -> Vec<u8> {
    let mut to_return: Vec<u8>;

    let port = addr.port();
    match addr.ip() {
        IpAddr::V4(ip) => {
            to_return = Vec::with_capacity(6);
            for byte in ip.octets() {
                to_return.push(byte);
            }
        }
        IpAddr::V6(ip) => {
            to_return = Vec::with_capacity(18);
            for byte in ip.octets() {
                to_return.push(byte);
            }
        }
    };

    to_return.push((port >> 8) as u8);
    to_return.push(port as u8);

    to_return
}

pub fn bin2addr(bin: &[u8]) -> Result<SocketAddr, AddressError> {
    match bin.len() {
        6 => {
            let port: u16 = bin[5] as u16 + ((bin[4] as u16) << 8);
            Ok(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(bin[0], bin[1], bin[2], bin[3])),
                port,
            ))
        }
        18 => {
            let port: u16 = bin[17] as u16 + ((bin[16] as u16) << 8);
            let octets: [u8; 16] = unsafe { bin[0..16].try_into().unwrap_unchecked() };
            let addr = SocketAddr::new(IpAddr::V6(Ipv6Addr::from(octets)), port);
            Ok(addr)
        }
        _ => Err(models_errors::AddressError::BadAddress),
    }
}

pub fn dump_addresses(addrs: &[SocketAddr]) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    let mut ipv4: Vec<u8> = Vec::new();
    let mut ipv6: Vec<u8> = Vec::new();

    for addr in addrs {
        let port = addr.port();
        match addr.ip() {
            IpAddr::V4(ip) => {
                for byte in ip.octets() {
                    ipv4.push(byte);
                }
                ipv4.push((port >> 8) as u8);
                ipv4.push(port as u8);
            }
            IpAddr::V6(ip) => {
                for byte in ip.octets() {
                    ipv6.push(byte);
                }
                ipv6.push((port >> 8) as u8);
                ipv6.push(port as u8);
            }
        }
    }

    let ipv4_to_return = match ipv4.len() {
        0 => None,
        _ => Some(ipv4),
    };

    let ipv6_to_return = match ipv6.len() {
        0 => None,
        _ => Some(ipv6),
    };

    (ipv4_to_return, ipv6_to_return)
}

pub fn parse_ipv4(data: &[u8]) -> Result<Vec<SocketAddr>, AddressError> {
    if data.len() % 6 != 0 {
        return Err(AddressError::WronSizeIPv4);
    }
    let mut to_return: Vec<SocketAddr> = Vec::with_capacity(data.len() / 6);

    for index in (0..data.len()).step_by(6) {
        let port: u16 = data[index + 5] as u16 + ((data[index + 4] as u16) << 8);
        let addr = SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(
                data[index],
                data[index + 1],
                data[index + 2],
                data[index + 3],
            )),
            port,
        );
        to_return.push(addr);
    }

    Ok(to_return)
}

pub fn parse_ipv6(data: &[u8]) -> Result<Vec<SocketAddr>, AddressError> {
    if data.len() % 18 != 0 {
        return Err(AddressError::WrongSizeIPv6);
    }
    let mut to_return: Vec<SocketAddr> = Vec::with_capacity(data.len() / 18);

    for index in (0..data.len()).step_by(18) {
        let port: u16 = data[index + 17] as u16 + ((data[index + 16] as u16) << 8);
        let octets: [u8; 16] = data[index..index + 16]
            .try_into()
            .map_err(|_| AddressError::WrongSizeIPv6)?;
        let addr = SocketAddr::new(IpAddr::V6(Ipv6Addr::from(octets)), port);
        to_return.push(addr);
    }

    Ok(to_return)
}

#[cfg(test)]
mod dump_parse_tests {
    use super::*;

    #[test]
    fn create_ping_packet() {
        let mut buf: Vec<u8> = Vec::new();
        let mut serializer = rmp_serde::Serializer::new(&mut buf);
        packet_models::Packet::Request(packet_models::Request::Ping(packet_models::PingRequest {
            id: 228,
        }))
        .serialize(&mut serializer)
        .unwrap();

        println!("{:X?}", buf);
    }

    #[test]
    fn create_ping_response() {
        let mut buf: Vec<u8> = Vec::new();
        let mut serializer = rmp_serde::Serializer::new(&mut buf);
        packet_models::Packet::Response(packet_models::Response::Ping(
            packet_models::PingResponse { id: 228 },
        ))
        .serialize(&mut serializer)
        .unwrap();

        println!("{:X?}", buf);
    }

    #[test]
    fn create_get_balance_request() {
        let mut buf: Vec<u8> = Vec::new();
        let mut serializer = rmp_serde::Serializer::new(&mut buf);
        packet_models::Packet::Request(packet_models::Request::GetAmount(
            packet_models::GetAmountRequest {
                id: 228,
                address: vec![1u8; 33],
            },
        ))
        .serialize(&mut serializer)
        .unwrap();

        println!("{:X?}", buf);
    }

    #[test]
    fn create_get_balance_response() {
        let mut buf: Vec<u8> = Vec::new();
        let mut serializer = rmp_serde::Serializer::new(&mut buf);
        packet_models::Packet::Response(packet_models::Response::GetAmount(
            packet_models::GetAmountResponse {
                id: 228,
                amount: vec![1u8; 33],
            },
        ))
        .serialize(&mut serializer)
        .unwrap();

        println!("{:X?}", buf);
    }

    #[test]
    fn create_getblocks_response() {
        let mut buf: Vec<u8> = Vec::new();
        let mut serializer = rmp_serde::Serializer::new(&mut buf);
        packet_models::Packet::Response(packet_models::Response::GetBlocks(
            packet_models::GetBlocksResponse {
                id: 228,
                blocks: vec![vec![1, 2], vec![1, 2]],
            },
        ))
        .serialize(&mut serializer)
        .unwrap();

        println!("{:X?}", buf);
    }

    #[test]
    fn create_submitpow_response() {
        let mut buf: Vec<u8> = Vec::new();
        let mut serializer = rmp_serde::Serializer::new(&mut buf);

        packet_models::Packet::Request(packet_models::Request::SubmitPow(
            packet_models::SubmitPow {
                id: 228,
                pow: vec![0],
                address: vec![1],
                timestamp: 45,
            },
        ))
        .serialize(&mut serializer)
        .unwrap();

        println!("{:X?}", buf);
    }

    #[test]
    fn dump_addresses_test() {
        let expected_ipv6 =
            b"\xfe\x80\xcd\x00\x00\x00\x0c\xde\x12\x57\x00\x00\x21\x1e\x72\x9c\x00\xff";
        let expected_ipv4 = b"\x7f\x00\x00\x01\x00\xff";
        let mut input: Vec<SocketAddr> = Vec::with_capacity(2);

        input.push(
            "[FE80:CD00:0000:0CDE:1257:0000:211E:729C]:255"
                .parse()
                .unwrap(),
        );
        input.push("127.0.0.1:255".parse().unwrap());

        let (ipv4, ipv6) = dump_addresses(&input);

        assert_eq!(expected_ipv4, ipv4.unwrap().as_slice());

        assert_eq!(expected_ipv6, ipv6.unwrap().as_slice());
    }

    #[test]
    fn parse_ipv4_test() {
        let mut expected: Vec<SocketAddr> = Vec::with_capacity(2);

        expected.push("127.0.0.1:255".parse().unwrap());
        expected.push("127.0.0.1:255".parse().unwrap());

        let dump_ipv4 = b"\x7f\x00\x00\x01\x00\xff\x7f\x00\x00\x01\x00\xff";

        let result = parse_ipv4(dump_ipv4).unwrap();

        assert_eq!(result, expected);
    }

    #[test]
    fn parse_ipv6_test() {
        let mut expected: Vec<SocketAddr> = Vec::with_capacity(2);

        expected.push(
            "[FE80:CD00:0000:0CDE:1257:0000:211E:729C]:255"
                .parse()
                .unwrap(),
        );
        expected.push(
            "[FE80:CD00:0000:0CDE:1257:0000:211E:729C]:255"
                .parse()
                .unwrap(),
        );

        let dump_ipv6 = b"\xfe\x80\xcd\x00\x00\x00\x0c\xde\x12\x57\x00\x00\x21\x1e\x72\x9c\x00\xff\xfe\x80\xcd\x00\x00\x00\x0c\xde\x12\x57\x00\x00\x21\x1e\x72\x9c\x00\xff";

        let result = parse_ipv6(dump_ipv6).unwrap();

        assert_eq!(result, expected);
    }
}
