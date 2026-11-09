//! Bounded HID short-item descriptor parser. Unsupported long items fail explicitly.
use crate::error::Fault;
use serde_json::{Value, json};
use std::collections::BTreeMap;
#[derive(Clone, Default)]
struct Globals {
    page: u32,
    min: i64,
    max: i64,
    size: u32,
    count: u32,
    id: u8,
}
pub fn parse(bytes: &[u8]) -> Result<Value, Fault> {
    let fail = |s: &str| Fault::new("invalid_descriptor", "hid.descriptor", s);
    if bytes.is_empty() || bytes.len() > 4096 {
        return Err(fail("descriptor length must be 1..4096"));
    }
    let (mut pos, mut depth) = (0, 0u32);
    let mut g = Globals::default();
    let mut stack = vec![];
    let (mut usages, mut umin, mut umax) = (vec![], None, None);
    let mut reports: BTreeMap<(u8, &str), (u64, Vec<Value>)> = BTreeMap::new();
    while pos < bytes.len() {
        let prefix = bytes[pos];
        pos += 1;
        if prefix == 0xfe {
            return Err(Fault::new(
                "unsupported",
                "hid.descriptor",
                "long items are not supported",
            ));
        }
        let size = match prefix & 3 {
            3 => 4,
            n => n as usize,
        };
        if pos + size > bytes.len() {
            return Err(fail("truncated item"));
        }
        let mut raw = [0u8; 4];
        raw[..size].copy_from_slice(&bytes[pos..pos + size]);
        pos += size;
        let value = u32::from_le_bytes(raw);
        let signed = if size == 0 {
            0
        } else {
            ((value as i32) << (32 - size * 8) >> (32 - size * 8)) as i64
        };
        let tag = prefix >> 4;
        match (prefix >> 2) & 3 {
            1 => match tag {
                0 => g.page = value,
                1 => g.min = signed,
                2 => g.max = if g.min < 0 { signed } else { value as i64 },
                7 => g.size = value,
                8 => {
                    if value == 0 || value > 255 {
                        return Err(fail("invalid report ID"));
                    }
                    g.id = value as u8;
                }
                9 => g.count = value,
                10 => {
                    if stack.len() >= 32 {
                        return Err(fail("global stack limit"));
                    }
                    stack.push(g.clone());
                }
                11 => g = stack.pop().ok_or_else(|| fail("global stack underflow"))?,
                _ => {}
            },
            2 => match tag {
                0 => {
                    if usages.len() >= 256 {
                        return Err(fail("usage limit"));
                    }
                    usages.push(value);
                }
                1 => umin = Some(value),
                2 => umax = Some(value),
                _ => {}
            },
            0 => {
                match tag {
                    8 | 9 | 11 => {
                        let kind = match tag {
                            8 => "input",
                            9 => "output",
                            _ => "feature",
                        };
                        let entry = reports.entry((g.id, kind)).or_default();
                        let bits = u64::from(g.size) * u64::from(g.count);
                        if bits == 0 || entry.0 + bits > 524288 || entry.1.len() >= 256 {
                            return Err(fail("invalid or excessive report size"));
                        }
                        entry.1.push(json!({"bit_offset":entry.0,"bit_size":g.size,"count":g.count,"flags":value,"usage_page":g.page,"usages":usages,"usage_min":umin,"usage_max":umax,"logical_min":g.min,"logical_max":g.max}));
                        entry.0 += bits;
                    }
                    10 => {
                        depth += 1;
                        if depth > 32 {
                            return Err(fail("collection depth limit"));
                        }
                    }
                    12 => {
                        depth = depth
                            .checked_sub(1)
                            .ok_or_else(|| fail("unmatched end collection"))?
                    }
                    _ => {}
                }
                usages.clear();
                umin = None;
                umax = None;
            }
            _ => {}
        }
    }
    if depth != 0 || !stack.is_empty() {
        return Err(fail("unclosed collection or global stack"));
    }
    let numbered = reports.keys().any(|(id, _)| *id != 0);
    if numbered && reports.keys().any(|(id, _)| *id == 0) {
        return Err(fail("mixed numbered and unnumbered reports"));
    }
    Ok(
        json!({"numbered_reports":numbered,"reports":reports.into_iter().map(|((id,kind),(bits,fields))|json!({"id":id,"type":kind,"bits":bits,"payload_bytes":bits.div_ceil(8),"wire_bytes":bits.div_ceil(8)+u64::from(numbered),"control_bytes":bits.div_ceil(8)+1,"fields":fields})).collect::<Vec<_>>()}),
    )
}
