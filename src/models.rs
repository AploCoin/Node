use crate::errors::*;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

pub mod packet_models {
    use super::*;

    #[repr(u8)]
    #[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize, Serialize)]
    pub enum ErrorCode {
        ParseError = 1,
    }

    #[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
    #[serde(tag = "type")]
    pub enum Packet {
        #[allow(non_camel_case_types)]
        request(Request),

        #[allow(non_camel_case_types)]
        response(Response),

        #[allow(non_camel_case_types)]
        error(ErrorR),
    }

    #[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
    #[serde(tag = "q")]
    pub enum Request {
        #[allow(non_camel_case_types)]
        get_nodes,

        #[allow(non_camel_case_types)]
        get_amount,

        #[allow(non_camel_case_types)]
        get_transaction,
    }

    #[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
    #[serde(tag = "r")]
    pub enum Response {
        #[allow(non_camel_case_types)]
        get_nodes(GetNodesReponse),

        #[allow(non_camel_case_types)]
        get_amount(GetAmountReponse),

        #[allow(non_camel_case_types)]
        get_transaction(GetTransactionResponse),
    }

    #[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
    pub struct ErrorR {
        code: ErrorCode,
    }

    #[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
    pub struct GetNodesReponse {
        ipv4: Option<Vec<u8>>,
        ipv6: Option<Vec<u8>>,
    }

    #[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
    pub struct GetAmountReponse {
        amount: Option<Vec<u8>>,
    }

    #[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
    pub struct GetTransactionResponse {
        transaction: Vec<u8>,
    }

    #[cfg(test)]
    mod packet_tests {
        use super::*;
        use rmp_serde::{Deserializer, Serializer};
        use std::io::Cursor;

        #[test]
        fn test_error() {
            let mut buf: Vec<u8> = Vec::new();

            let obj = Packet::error(ErrorR {
                code: ErrorCode::ParseError,
            });

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

pub fn dump_addresses(addrs: &[SocketAddr]) -> (Vec<u8>, Vec<u8>) {
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

    (ipv4, ipv6)
}

pub fn parse_ipv4(data: &[u8]) -> Result<Vec<SocketAddr>> {
    if data.len() % 6 != 0 {
        return Err(models_errors::WrongSizeIPv4.into());
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

pub fn parse_ipv6(data: &[u8]) -> Result<Vec<SocketAddr>> {
    if data.len() % 18 != 0 {
        return Err(models_errors::WrongSizeIPv6.into());
    }
    let mut to_return: Vec<SocketAddr> = Vec::with_capacity(data.len() / 18);

    for index in (0..data.len()).step_by(18) {
        let port: u16 = data[index + 17] as u16 + ((data[index + 16] as u16) << 8);
        let octets: [u8; 16] = data[index..index + 16].try_into()?;
        let addr = SocketAddr::new(IpAddr::V6(Ipv6Addr::from(octets)), port);
        to_return.push(addr);
    }

    Ok(to_return)
}

#[cfg(test)]
mod dump_parse_tests {
    use super::*;

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

        assert_eq!(expected_ipv4, ipv4.as_slice());

        assert_eq!(expected_ipv6, ipv6.as_slice());
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
