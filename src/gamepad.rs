//! Read-only XInput battery primitives. Slots are never inferred from VID/PID.
use crate::error::Fault;
use serde_json::Value;
#[cfg(any(windows, test))]
use serde_json::json;

pub fn battery_info(slot: u32) -> Result<Value, Fault> {
    if slot > 3 {
        return Err(Fault::new(
            "invalid_argument",
            "gamepad.battery_info",
            "XInput slot must be 0..3",
        ));
    }
    #[cfg(windows)]
    {
        use windows::Win32::UI::Input::XboxController::{
            BATTERY_DEVTYPE_GAMEPAD, XINPUT_BATTERY_INFORMATION, XInputGetBatteryInformation,
        };
        let mut value = XINPUT_BATTERY_INFORMATION::default();
        let code =
            unsafe { XInputGetBatteryInformation(slot, BATTERY_DEVTYPE_GAMEPAD, &mut value) };
        if code == 1167 {
            return Ok(json!({"slot":slot,"source":"xinput","connected":false,"available":false}));
        }
        if code != 0 {
            return Err(Fault::new(
                "backend",
                "gamepad.battery_info",
                format!("Win32 error {code}"),
            ));
        }
        Ok(decode(slot, value.BatteryType.0, value.BatteryLevel.0))
    }
    #[cfg(not(windows))]
    Err(Fault::new(
        "unsupported",
        "gamepad.battery_info",
        "XInput requires Windows",
    ))
}
#[cfg(any(windows, test))]
fn decode(slot: u32, kind: u8, level: u8) -> Value {
    let level_valid = matches!(kind, 2 | 3 | 255) && level <= 3;
    json!({"slot":slot,"source":"xinput","connected":true,"available":kind!=0,"precision":"coarse",
        "battery_type_code":kind,"battery_type":match kind{0=>"disconnected",1=>"wired",2=>"alkaline",3=>"nimh",255=>"unknown",_=>"unrecognized"},
        "level_code":level,"level":if level_valid {Some(match level{0=>"empty",1=>"low",2=>"medium",3=>"full",_=>unreachable!()})}else{None},
        "percent":null,"charging":null,"identity":"xinput_slot_only"})
}
pub fn install(lua: &mlua::Lua) -> mlua::Result<()> {
    use mlua::LuaSerdeExt;
    let table = lua.create_table()?;
    table.set(
        "battery_info",
        lua.create_async_function(|lua, slot: u32| async move {
            let value = tokio::task::spawn_blocking(move || battery_info(slot))
                .await
                .map_err(mlua::Error::external)?
                .map_err(Fault::lua)?;
            lua.to_value(&value)
        })?,
    )?;
    lua.globals().set("gamepad", table)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn coarse_levels_never_claim_percent_or_charging() {
        for kind in [0, 1, 2, 3, 255] {
            for level in 0..=5 {
                let r = decode(0, kind, level);
                assert!(r["percent"].is_null());
                assert!(r["charging"].is_null());
                if kind == 1 || kind == 0 || level > 3 {
                    assert!(r["level"].is_null());
                }
            }
        }
        assert_eq!(decode(1, 2, 2)["level"], "medium");
    }
}
