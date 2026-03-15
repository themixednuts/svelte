use crate::Result;

#[derive(Clone, Copy, Debug)]
pub enum HashValue<'a> {
    Str(&'a str),
    Bytes(&'a [u8]),
}

pub fn hash_values<'a>(values: impl IntoIterator<Item = HashValue<'a>>) -> Result<String> {
    let mut hash = 5381_u32;

    for value in values {
        match value {
            HashValue::Str(value) => {
                let mut index = value.len();
                while index != 0 {
                    index -= 1;
                    hash = hash.wrapping_mul(33) ^ u32::from(value.as_bytes()[index]);
                }
            }
            HashValue::Bytes(value) => {
                let mut index = value.len();
                while index != 0 {
                    index -= 1;
                    hash = hash.wrapping_mul(33) ^ u32::from(value[index]);
                }
            }
        }
    }

    Ok(hash.to_string_radix(36))
}

trait ToStringRadix {
    fn to_string_radix(self, radix: u32) -> String;
}

impl ToStringRadix for u32 {
    fn to_string_radix(self, radix: u32) -> String {
        debug_assert!((2..=36).contains(&radix));
        if self == 0 {
            return "0".to_string();
        }

        let mut value = self;
        let mut digits = Vec::new();
        while value != 0 {
            let digit = (value % radix) as u8;
            digits.push(if digit < 10 {
                (b'0' + digit) as char
            } else {
                (b'a' + (digit - 10)) as char
            });
            value /= radix;
        }
        digits.iter().rev().collect()
    }
}
