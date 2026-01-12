//! Processing module - Contains BOTTLENECK #1 (CPU)

use crate::app::Request;
use crate::utils;

pub struct RequestProcessor {
    transform_buffer: Vec<u8>,
}

impl RequestProcessor {
    pub fn new() -> Self {
        Self {
            transform_buffer: Vec::with_capacity(1024),
        }
    }

    #[inline(never)]
    pub fn process(&mut self, request: &Request) -> Vec<u8> {
        // Transform the payload
        let transformed = self.transform(&request.payload);

        // Compute result based on priority
        match request.priority {
            0 => self.compute_fast(&transformed),
            1 => self.compute_medium(&transformed),
            _ => self.compute_slow(&transformed),
        }
    }

    #[inline(never)]
    fn transform(&mut self, data: &[u8]) -> Vec<u8> {
        // Looks innocent... uses "safe" clone
        let safe_data = utils::safe_clone_bytes(data);
        self.transform_buffer.clear();
        self.transform_buffer.extend_from_slice(&safe_data);

        // BOTTLENECK #1: Unnecessary repeated hashing
        // This does way more work than needed
        for _ in 0..50 {
            let hash = self.hash_data(&self.transform_buffer);
            self.transform_buffer.push((hash & 0xFF) as u8);
            self.transform_buffer.truncate(data.len());
        }

        self.transform_buffer.clone()
    }

    #[inline(never)]
    fn hash_data(&self, data: &[u8]) -> u64 {
        // Deliberately slow hash for CTF
        let mut hash = 0u64;
        for (i, &byte) in data.iter().enumerate() {
            for j in 0..100 {
                hash = hash.wrapping_mul(31).wrapping_add(byte as u64);
                hash ^= (i as u64).wrapping_mul(j as u64);
            }
        }
        hash
    }

    #[inline(never)]
    fn compute_fast(&self, data: &[u8]) -> Vec<u8> {
        data.iter().map(|b| b.wrapping_add(1)).collect()
    }

    #[inline(never)]
    fn compute_medium(&self, data: &[u8]) -> Vec<u8> {
        let mut result: Vec<u8> = data.to_vec();
        result.sort();
        result
    }

    #[inline(never)]
    fn compute_slow(&self, data: &[u8]) -> Vec<u8> {
        // This looks slow but isn't the main bottleneck
        let mut result = Vec::with_capacity(data.len());
        for chunk in data.chunks(8) {
            let sum: u16 = chunk.iter().map(|&b| b as u16).sum();
            result.push((sum % 256) as u8);
        }
        result
    }
}
