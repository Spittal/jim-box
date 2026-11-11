//! The network's weights, as written by `scripts/export-beat-this.py`.

use std::collections::HashMap;

const MAGIC: &[u8] = b"BTHS1\n";

pub struct Tensor {
    pub shape: Vec<usize>,
    pub data: Vec<f32>,
}

pub struct Weights(HashMap<String, Tensor>);

impl Weights {
    /// Panics on a file that isn't one: the weights are compiled in, so a
    /// bad one is a build mistake.
    pub fn parse(bytes: &[u8]) -> Self {
        assert!(bytes.starts_with(MAGIC), "not a Beat This weights file");
        let mut rest = &bytes[MAGIC.len()..];
        let mut take = |n: usize| {
            let (taken, after) = rest.split_at(n);
            rest = after;
            taken
        };
        let mut tensors = HashMap::new();
        let mut left = bytes.len() - MAGIC.len();
        while left > 0 {
            let length = take(2);
            let length = usize::from(u16::from_le_bytes([length[0], length[1]]));
            let name = String::from_utf8(take(length).to_vec()).expect("tensor name");
            let dimensions = usize::from(take(1)[0]);
            let shape: Vec<usize> =
                (0..dimensions).map(|_| u32::from_le_bytes(take(4).try_into().expect("four bytes")) as usize).collect();
            let count: usize = shape.iter().product();
            let data = take(count * 4).chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
            left -= 2 + length + 1 + dimensions * 4 + count * 4;
            tensors.insert(name, Tensor { shape, data });
        }
        Self(tensors)
    }

    pub fn tensor(&self, name: &str) -> &Tensor {
        self.0.get(name).unwrap_or_else(|| panic!("weights have no {name}"))
    }

    pub fn get(&self, name: &str) -> Vec<f32> {
        self.tensor(name).data.clone()
    }

    pub fn has(&self, name: &str) -> bool {
        self.0.contains_key(name)
    }
}
