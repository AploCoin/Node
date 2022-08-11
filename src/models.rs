use serde::{Deserialize, Serialize};

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Debug, Deserialize, Serialize)]
pub enum ErrorCode{
    ParseError = 1
}

#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum Packet{
    #[allow(non_camel_case_types)]
    request(Request),

    #[allow(non_camel_case_types)]
    response(Response),
    
    #[allow(non_camel_case_types)]
    error(ErrorR)
} 

#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "q")]
pub enum Request{
    #[allow(non_camel_case_types)]
    get_nodes,

    #[allow(non_camel_case_types)]
    get_amount,

    #[allow(non_camel_case_types)]
    get_transaction
}

#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "r")]
pub enum Response{
    #[allow(non_camel_case_types)]
    get_nodes(GetNodesReponse),

    #[allow(non_camel_case_types)]
    get_amount(GetAmountReponse),

    #[allow(non_camel_case_types)]
    get_transaction(GetTransactionResponse)
}

#[derive(Debug, PartialEq, Deserialize, Serialize)]
pub struct ErrorR{
    code:ErrorCode
}


#[derive(Debug, PartialEq, Deserialize, Serialize)]
pub struct GetNodesReponse{
    ipv4: Option<Vec<u8>>,
    ipv6: Option<Vec<u8>>
}

#[derive(Debug, PartialEq, Deserialize, Serialize)]
pub struct GetAmountReponse{
    ipv4: Option<Vec<u8>>
}

#[derive(Debug, PartialEq, Deserialize, Serialize)]
pub struct GetTransactionResponse{
    transaction: Vec<u8>
}

#[cfg(test)]
mod packet_tests{
    use super::*;
    use rmp_serde::{Deserializer, Serializer};
    use std::io::Cursor;

    #[test]
    fn test_error(){
        let mut buf:Vec<u8> = Vec::new();

        let obj = Packet::error(ErrorR{code:ErrorCode::ParseError});

        obj.serialize(&mut Serializer::new(&mut buf)).unwrap();

        let deserialized = Packet::deserialize(&mut Deserializer::new(Cursor::new(buf))).unwrap();

        assert_eq!(obj,deserialized);
    }
}
