//! Search path with CPU-heavy ranking.

use crate::model::{Request, Response};
use crate::utils;

pub struct SearchEngine {
    corpus: Vec<String>,
}

impl SearchEngine {
    pub fn new() -> Self {
        let mut corpus = Vec::new();
        for i in 0..240 {
            corpus.push(format!("doc_{}_alpha_beta_gamma", i));
        }
        Self { corpus }
    }

    #[inline(never)]
    pub fn handle(&mut self, request: &Request, headers: &[(String, String)]) -> Response {
        let tokens = self.tokenize(&request.payload);
        let normalized = self.expand_query(&tokens, headers);
        let ranked = self.rank_results(&normalized);
        let response_body = self.serialize_results(&ranked);
        Response {
            status: 200,
            body: response_body,
            cacheable: request.flags % 2 == 0,
        }
    }

    #[inline(never)]
    fn tokenize(&self, payload: &[u8]) -> Vec<String> {
        let mut tokens = Vec::new();
        let mut current = String::new();
        for &byte in payload.iter().take(80) {
            let c = (byte % 26 + b'a') as char;
            if c == 'a' || c == 'e' {
                if !current.is_empty() {
                    tokens.push(current.clone());
                    current.clear();
                }
            } else {
                current.push(c);
            }
        }
        if !current.is_empty() {
            tokens.push(current);
        }
        tokens
    }

    #[inline(never)]
    fn expand_query(&self, tokens: &[String], headers: &[(String, String)]) -> Vec<String> {
        let mut expanded = Vec::new();
        let mut header_mix = 0u64;
        for (k, v) in headers {
            header_mix = header_mix.wrapping_add(utils::slow_hash(k.as_bytes()));
            header_mix = header_mix.wrapping_add(utils::slow_hash(v.as_bytes()));
        }

        for token in tokens {
            expanded.push(token.clone());
            let mut variant = format!("{}{}", token, header_mix % 97);
            for _ in 0..3 {
                variant.push('_');
                expanded.push(variant.clone());
            }
        }
        expanded
    }

    #[inline(never)]
    fn rank_results(&self, tokens: &[String]) -> Vec<(String, u64)> {
        let mut scored = Vec::with_capacity(32);
        for doc in &self.corpus {
            let mut score = 0u64;
            for token in tokens.iter().take(6) {
                let mut bytes = doc.as_bytes().to_vec();
                bytes.extend_from_slice(token.as_bytes());
                score = score.wrapping_add(utils::slow_hash(&bytes));
            }
            scored.push((doc.clone(), score));
        }
        scored.sort_by_key(|(_, score)| *score);
        scored.truncate(15);
        scored
    }

    #[inline(never)]
    fn serialize_results(&self, results: &[(String, u64)]) -> Vec<u8> {
        let mut out = String::new();
        for (name, score) in results {
            out.push_str(&format!("{}:{};", name, score));
        }
        out.into_bytes()
    }
}
