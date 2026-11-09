use crate::{
    bindings::{Buffer, unhex},
    error::Fault,
};
use mlua::{AnyUserData, Lua, Table, UserDataMethods};

fn bad(message: &str) -> mlua::Error {
    Fault::new("invalid_argument", "buffer", message).lua()
}
fn range(b: &Buffer, offset: usize, size: usize) -> mlua::Result<&[u8]> {
    b.bytes()
        .get(
            offset
                ..offset
                    .checked_add(size)
                    .ok_or_else(|| bad("offset overflow"))?,
        )
        .ok_or_else(|| bad("Buffer offset out of range (0-based)"))
}
pub fn methods<M: UserDataMethods<Buffer>>(m: &mut M) {
    m.add_method("string", |lua, b, ()| lua.create_string(b.bytes()));
    m.add_method("equals", |_, b, other: AnyUserData| {
        Ok(b.bytes() == other.borrow::<Buffer>()?.bytes())
    });
    m.add_method("concat", |_, b, other: AnyUserData| {
        let other = other.borrow::<Buffer>()?;
        if b.bytes().len() + other.bytes().len() > 65536 {
            return Err(bad("buffer too large"));
        }
        let mut out = b.bytes().to_vec();
        out.extend_from_slice(other.bytes());
        Buffer::new(out, b.budget())
    });
    m.add_method("replace", |_, b, (offset, data): (usize, AnyUserData)| {
        let data = data.borrow::<Buffer>()?;
        range(b, offset, data.bytes().len())?;
        let mut out = b.bytes().to_vec();
        out[offset..offset + data.bytes().len()].copy_from_slice(data.bytes());
        Buffer::new(out, b.budget())
    });
    m.add_method(
        "uint",
        |_, b, (offset, width, endian): (usize, usize, Option<String>)| {
            if ![1, 2, 3, 4, 8].contains(&width) {
                return Err(bad("integer width must be 1,2,3,4,8"));
            }
            let bytes = range(b, offset, width)?;
            let little = endian.as_deref().unwrap_or("le");
            let mut out = 0u64;
            match little {
                "le" => {
                    for (i, v) in bytes.iter().enumerate() {
                        out |= (*v as u64) << (i * 8)
                    }
                }
                "be" => {
                    for v in bytes {
                        out = (out << 8) | (*v as u64)
                    }
                }
                _ => return Err(bad("endianness must be le or be")),
            };
            if out > i64::MAX as u64 {
                return Err(bad(
                    "unsigned value exceeds Lua integer; use hex/string for u64 > 2^63-1",
                ));
            }
            Ok(out as i64)
        },
    );
    m.add_method(
        "int",
        |_, b, (offset, width, endian): (usize, usize, Option<String>)| {
            if ![1, 2, 3, 4, 8].contains(&width) {
                return Err(bad("integer width must be 1,2,3,4,8"));
            }
            let bytes = range(b, offset, width)?;
            let mut out = 0u64;
            match endian.as_deref().unwrap_or("le") {
                "le" => {
                    for (i, v) in bytes.iter().enumerate() {
                        out |= (*v as u64) << (i * 8)
                    }
                }
                "be" => {
                    for v in bytes {
                        out = (out << 8) | (*v as u64)
                    }
                }
                _ => return Err(bad("endianness must be le or be")),
            };
            let shift = 64 - width * 8;
            Ok(((out << shift) as i64) >> shift)
        },
    );
    m.add_method(
        "float",
        |_, b, (offset, width, endian): (usize, usize, Option<String>)| {
            let bytes = range(b, offset, width)?;
            let le = match endian.as_deref().unwrap_or("le") {
                "le" => true,
                "be" => false,
                _ => return Err(bad("endianness must be le or be")),
            };
            match width {
                4 => {
                    let a: [u8; 4] = bytes.try_into().unwrap();
                    Ok(if le {
                        f32::from_le_bytes(a)
                    } else {
                        f32::from_be_bytes(a)
                    } as f64)
                }
                8 => {
                    let a: [u8; 8] = bytes.try_into().unwrap();
                    Ok(if le {
                        f64::from_le_bytes(a)
                    } else {
                        f64::from_be_bytes(a)
                    })
                }
                _ => Err(bad("float width must be 4 or 8")),
            }
        },
    );
}
pub fn install(
    lua: &Lua,
    table: &Table,
    budget: &std::sync::Arc<crate::bindings::Budget>,
) -> mlua::Result<()> {
    let b = budget.clone();
    table.set(
        "string",
        lua.create_function(move |_, s: mlua::LuaString| Buffer::new(s.as_bytes().to_vec(), &b))?,
    )?;
    let b = budget.clone();
    table.set(
        "alloc",
        lua.create_function(move |_, (len, fill): (usize, Option<u8>)| {
            if len > 65536 {
                return Err(bad("buffer too large"));
            }
            Buffer::new(vec![fill.unwrap_or(0); len], &b)
        })?,
    )?;
    let b = budget.clone();
    table.set(
        "integer",
        lua.create_function(
            move |_, (value, width, endian, signed): (i64, usize, Option<String>, Option<bool>)| {
                if ![1, 2, 3, 4, 8].contains(&width) {
                    return Err(bad("integer width must be 1,2,3,4,8"));
                }
                let bits = width * 8;
                let valid = if signed.unwrap_or(false) {
                    bits == 64 || (value >= -(1i64 << (bits - 1)) && value < (1i64 << (bits - 1)))
                } else {
                    value >= 0 && (bits == 64 || (value as u64) < (1u64 << bits))
                };
                if !valid {
                    return Err(bad("integer out of range"));
                }
                let bytes = match endian.as_deref().unwrap_or("le") {
                    "le" => value.to_le_bytes()[..width].to_vec(),
                    "be" => value.to_be_bytes()[8 - width..].to_vec(),
                    _ => return Err(bad("endianness must be le or be")),
                };
                Buffer::new(bytes, &b)
            },
        )?,
    )?;
    let b = budget.clone();
    table.set(
        "float",
        lua.create_function(
            move |_, (value, width, endian): (f64, usize, Option<String>)| {
                let le = match endian.as_deref().unwrap_or("le") {
                    "le" => true,
                    "be" => false,
                    _ => return Err(bad("endianness must be le or be")),
                };
                let bytes = match width {
                    4 => {
                        if le {
                            (value as f32).to_le_bytes().to_vec()
                        } else {
                            (value as f32).to_be_bytes().to_vec()
                        }
                    }
                    8 => {
                        if le {
                            value.to_le_bytes().to_vec()
                        } else {
                            value.to_be_bytes().to_vec()
                        }
                    }
                    _ => return Err(bad("float width must be 4 or 8")),
                };
                Buffer::new(bytes, &b)
            },
        )?,
    )?;
    let b = budget.clone();
    table.set(
        "decode",
        lua.create_function(move |_, s: String| Buffer::new(unhex(&s)?, &b))?,
    )?;
    Ok(())
}
