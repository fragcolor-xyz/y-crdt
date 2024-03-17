use std::convert::TryFrom;
use std::hash::Hasher;

// An minimalistic hasher for client-ids!
// Client-ids are already random only have a size of maximum 64 bit. No reason to spin up
// cryptographic functions every time a client is queried.
#[derive(Default)]
pub struct ClientHasher {
    prefix: u64,
}

impl Hasher for ClientHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.prefix
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        debug_assert!(bytes.len() == 8);
        let _ = u64::from_ne_bytes(<[u8; 8]>::try_from(bytes).unwrap());
    }
}
