use serde::Deserialize;
use serde_json_core::{from_slice, to_slice};

use crate::{Buffer, BufferError, BufferReader, BufferWriter};

pub trait JsonWriter {
    fn serialize_json<T: serde::Serialize>(&mut self, src: &T) -> Result<usize, BufferError>;
}

impl <'a, W: BufferWriter> JsonWriter for W {
    fn serialize_json<T: serde::Serialize>(&mut self, src: &T) -> Result<usize, BufferError> {
        
        let n = to_slice(src, self)
            .map_err(|_e| BufferError::NoCapacity)?;

        self.commit(n)?;
        Ok(n)
    }
}

impl <S: AsMut<[u8]> + AsRef<[u8]>> JsonWriter for Buffer<S> {
    fn serialize_json<T: serde::Serialize>(&mut self, src: &T) -> Result<usize, BufferError> {
        
        let tgt = &mut self.source.as_mut()[self.write_position..];
        
        let n = to_slice(src, tgt)
            .map_err(|_e| BufferError::NoCapacity)?;

        self.write_position += n;
        Ok(n)
    }
}
pub trait JsonReader<'a> {
    fn deserialize_json<'de, T: Deserialize<'de>>(&'de mut self) -> Result<T, BufferError> where 'a: 'de;
}


impl <'a, R: BufferReader> JsonReader<'a> for R {
    fn deserialize_json<'de, T: Deserialize<'de>>(&'de mut self) -> Result<T, BufferError> where 'a: 'de {
        
        let (res, n) = from_slice::<'de, T>(self)
            .map_err(|e| BufferError::JsonDeserialize(e))?;

        self.add_bytes_read(n);

        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use core::str::{self, from_utf8};

    use serde::{Deserialize, Serialize};

    use crate::{Buffer, ReadWrite};

    use super::{JsonReader, JsonWriter};


    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct DummyJson {
        a: usize
    }

    #[test]
    fn test_serialize_json() {

        let d = DummyJson{ a: 4 };

        let mut b = [0u8; 64];
        let mut buf = Buffer::new(&mut b);

        buf.serialize_json(&d).unwrap();

        const EXPECTED_JSON: &'static str = "{\"a\":4}";

        let json = from_utf8(buf.data()).unwrap();

        assert_eq!(json, EXPECTED_JSON);
    }

    #[test]
    fn test_deserialize_json() {

        const JSON: &'static str = "{\"a\":7}";

        let mut b = [0u8; 64];
        let mut buf = Buffer::new(&mut b);

        buf.write_base(JSON.as_bytes()).unwrap();

        let mut reader = buf.create_reader();

        let res = reader.deserialize_json::<'_, DummyJson>().unwrap();

        assert_eq!(res.a, 7);
        drop(reader);

        assert_eq!(buf.read_position, JSON.len());
    }

    // Not supported by lib
    /*
    #[test]
    fn test_multi_deserialize_json() {

        const JSON_1: &'static str = "{\"a\":9}";
        const JSON_2: &'static str = "{\"a\":234}";

        let mut b = [0u8; 64];
        let mut buf = Buffer::new(&mut b);

        buf.write_base(JSON_1.as_bytes()).unwrap();
        buf.write_base(JSON_2.as_bytes()).unwrap();

        let mut reader = buf.create_reader();

        let res = reader.deserialize_json::<'_, DummyJson>().unwrap();

        assert_eq!(res.a, 9);
        drop(reader);

        assert_eq!(buf.read_position, JSON_1.len());
    }
    */

}