use std::str;

use datasize::DataSize;

#[derive(Default, Clone, Debug, DataSize)]
pub struct IndexedPackedStrings {
    // each element stores a pointer and length into the `backing_store`
    // the pointer is in the upper 40 bits, and the length is in the lower 24 bits
    data: Vec<u64>,
    backing_store: Vec<u8>,
}

impl IndexedPackedStrings {
    pub fn push(&mut self, elem: &str) {
        let bytes = elem.as_bytes();
        // TODO(34): overflow
        self.data
            .push(((self.backing_store.len() << 24) + bytes.len()) as u64);
        self.backing_store.extend_from_slice(bytes);
    }

    pub fn clear(&mut self) {
        self.data.clear();
        self.backing_store.clear();
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> + Clone {
        self.data.iter().map(move |&offset_len| {
            let offset = (offset_len >> 24) as usize;
            let len = (offset_len & 0x00ff_ffff) as usize;
            unsafe { str::from_utf8_unchecked(&self.backing_store[offset..(offset + len)]) }
        })
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn into_parts(self) -> (Vec<u64>, Vec<u8>) {
        (self.data, self.backing_store)
    }
}

pub struct PackedStrings {
    data: Vec<u8>,
}

// PERF: encode using variable size length
impl PackedStrings {
    pub fn from_iterator<'a>(strings: impl Iterator<Item = &'a str>) -> PackedStrings {
        let mut sp = PackedStrings { data: Vec::new() };
        for string in strings {
            sp.push(string);
        }
        sp.shrink_to_fit();
        sp
    }

    pub fn push(&mut self, string: &str) {
        let b = string.as_bytes();
        let mut len = b.len();
        while len > 254 {
            self.data.push(255);
            len -= 255;
        }
        self.data.push(len as u8);
        self.data.extend_from_slice(b);
    }

    pub fn shrink_to_fit(&mut self) {
        self.data.shrink_to_fit();
    }

    pub fn into_vec(self) -> Vec<u8> {
        self.data
    }
}

pub struct StringPackerIterator<'a> {
    data: &'a [u8],
    curr_index: usize,
}

impl<'a> StringPackerIterator<'a> {
    /// `data` must be valid encoding for StringPacker
    pub unsafe fn from_slice(data: &'a [u8]) -> StringPackerIterator<'a> {
        StringPackerIterator {
            data,
            curr_index: 0,
        }
    }
}

impl<'a> Iterator for StringPackerIterator<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        if self.curr_index >= self.data.len() {
            return None;
        }

        let mut len = 0usize;
        while self.data[self.curr_index] == 255 {
            len += 255;
            self.curr_index += 1;
        }
        len += self.data[self.curr_index] as usize;
        self.curr_index += 1;

        let result =
            unsafe { str::from_utf8_unchecked(&self.data[self.curr_index..self.curr_index + len]) };
        self.curr_index += len;
        Some(result)
    }
}

// Could unify with PackedStrings
pub struct PackedBytes {
    data: Vec<u8>,
}

impl PackedBytes {
    pub fn from_iterator(bytes: impl Iterator<Item = Vec<u8>>) -> PackedBytes {
        let mut data = Vec::<u8>::new();
        for b in bytes {
            let mut len = b.len();
            while len > 254 {
                data.push(255);
                len -= 255;
            }
            data.push(len as u8);
            data.extend_from_slice(&b);
        }
        data.shrink_to_fit();
        PackedBytes { data }
    }

    pub fn into_vec(self) -> Vec<u8> {
        self.data
    }
}

pub struct PackedBytesIterator<'a> {
    data: &'a [u8],
    curr_index: usize,
}

impl<'a> PackedBytesIterator<'a> {
    pub fn from_slice(data: &'a [u8]) -> PackedBytesIterator<'a> {
        PackedBytesIterator {
            data,
            curr_index: 0,
        }
    }

    pub fn has_more(&self) -> bool {
        self.curr_index < self.data.len()
    }
}

impl<'a> Iterator for PackedBytesIterator<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        if !self.has_more() {
            return None;
        }

        let mut index = self.curr_index;
        let mut len = 0usize;
        while self.data[index] == 255 {
            len += 255;
            index += 1;
        }
        len += self.data[index] as usize;
        index += 1;

        let result = &self.data[index..(index + len)];
        self.curr_index = index + len;
        Some(result)
    }
}
