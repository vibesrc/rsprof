//! Checkout path with moderate CPU and allocation churn.

use crate::model::{Request, Response};
use crate::utils;

pub struct CheckoutEngine {
    pricing_table: Vec<u64>,
}

impl CheckoutEngine {
    pub fn new() -> Self {
        let mut pricing_table = Vec::new();
        for i in 0..512 {
            pricing_table.push(199 + (i as u64 * 7) % 97);
        }
        Self { pricing_table }
    }

    #[inline(never)]
    pub fn handle(&mut self, request: &Request, _headers: &[(String, String)]) -> Response {
        let items = self.expand_cart(request.payload.len() as u64);
        let total = self.compute_total(&items);
        let discounted = self.apply_promos(total, request.flags);
        let body = self.serialize_receipt(&items, discounted);
        Response {
            status: 201,
            body,
            cacheable: false,
        }
    }

    #[inline(never)]
    fn expand_cart(&self, seed: u64) -> Vec<u64> {
        let mut items = Vec::new();
        let mut idx = seed as usize % self.pricing_table.len();
        for _ in 0..12 {
            items.push(self.pricing_table[idx]);
            idx = (idx * 13 + 7) % self.pricing_table.len();
        }
        items
    }

    #[inline(never)]
    fn compute_total(&self, items: &[u64]) -> u64 {
        let mut total = 0u64;
        for &price in items {
            for _ in 0..65 {
                total = total.wrapping_add(price);
                total = total.wrapping_mul(31).wrapping_add(17);
            }
        }
        total
    }

    #[inline(never)]
    fn apply_promos(&self, total: u64, flags: u8) -> u64 {
        let mut value = total;
        if flags & 1 == 1 {
            value = value.saturating_sub(total / 10);
        }
        if flags & 2 == 2 {
            value = value.saturating_sub(total / 20);
        }
        value
    }

    #[inline(never)]
    fn serialize_receipt(&self, items: &[u64], total: u64) -> Vec<u8> {
        let mut out = String::new();
        for &item in items {
            out.push_str(&format!("{}|", item));
        }
        out.push_str(&format!("total={}", utils::format_bytes(total as usize)));
        out.into_bytes()
    }
}
