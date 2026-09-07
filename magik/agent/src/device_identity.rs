//! The board Ethernet MAC survives service replacement and DHCP changes.

use std::{fs, io};

pub fn read() -> io::Result<String> {
    normalize(&fs::read_to_string("/sys/class/net/eth0/address")?)
}

fn normalize(value: &str) -> io::Result<String> {
    let value = value.trim().to_ascii_lowercase();
    if value.len() != 17
        || value == "00:00:00:00:00:00"
        || value == "ff:ff:ff:ff:ff:ff"
        || !value.bytes().enumerate().all(|(i, b)| {
            if i % 3 == 2 {
                b == b':'
            } else {
                b.is_ascii_hexdigit()
            }
        })
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid board identity",
        ));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_board_identity() {
        assert_eq!(
            normalize("02:AB:34:56:78:90\n").unwrap(),
            "02:ab:34:56:78:90"
        );
        for value in [
            "",
            "00:00:00:00:00:00",
            "ff:ff:ff:ff:ff:ff",
            "02:xx:34:56:78:90",
        ] {
            assert!(normalize(value).is_err());
        }
    }
}
