use super::*;
#[derive(Clone)]
pub(super) struct Channel {
    pub(super) core: Arc<Core>,
    pub(super) budget: Arc<bindings::Budget>,
}
impl Resource for Channel {
    fn close(&self) {
        self.core.close()
    }
    fn closed(&self) -> bool {
        self.core.closed()
    }
}
#[derive(Clone)]
pub(super) struct Stream {
    pub(super) channel: Channel,
    pub(super) queue: Arc<Queue>,
}
fn packet(lua: &Lua, value: Value, budget: &Arc<bindings::Budget>) -> mlua::Result<Table> {
    let LV::Table(table) = lua.to_value(&value)? else {
        return Err(Fault::new("invalid_frame", "gip.receive", "event must be an object").lua());
    };
    if value["kind"] == "message" {
        table.set(
            "age_ms",
            super::super::diagnostics::monotonic_ms()
                .saturating_sub(value["received_monotonic_ms"].as_u64().unwrap_or(0)),
        )?;
        table.set(
            "payload",
            Buffer::new(
                bindings::unhex(value["payload_hex"].as_str().unwrap_or(""))?,
                budget,
            )?,
        )?;
    }
    Ok(table)
}
impl UserData for Stream {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_async_method("next", |lua, s, timeout: Option<u64>| async move {
            s.channel.core.ensure_open().map_err(Fault::lua)?;
            let value = s
                .queue
                .next(timeout.unwrap_or(30000))
                .await
                .map_err(Fault::lua)?;
            s.channel.core.ensure_open().map_err(Fault::lua)?;
            packet(&lua, value, &s.channel.budget)
        });
        m.add_method("close", |_, s, ()| {
            s.queue.close();
            Ok(())
        });
        m.add_method("inspect", |lua, s, ()| lua.to_value(&s.queue.info()));
    }
}
impl UserData for Channel {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("inspect", |lua, c, ()| lua.to_value(&c.core.inspect()));
        m.add_async_method("close", |_, c, ()| async move {
            c.core.close();
            let deadline = Instant::now() + Duration::from_secs(2);
            while !c.core.closed() && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            if !c.core.closed() {
                return Err(Fault::new(
                    "stalled",
                    "gip.close",
                    "native operation still pending; device lease retained",
                )
                .lua());
            }
            Ok(())
        });
        m.add_method("subscribe", |lua, c, filter: Option<Table>| {
            let filter: MessageFilter = filter
                .map(|f| lua.from_value(LV::Table(f)))
                .transpose()?
                .unwrap_or_default();
            Ok(Stream {
                channel: c.clone(),
                queue: c.core.subscribe(filter).map_err(Fault::lua)?,
            })
        });
        m.add_async_method("read_message", |lua, c, timeout: Option<u64>| async move {
            c.core.ensure_open().map_err(Fault::lua)?;
            let value = c
                .core
                .input
                .next(timeout.unwrap_or(30000))
                .await
                .map_err(Fault::lua)?;
            c.core.ensure_open().map_err(Fault::lua)?;
            packet(&lua, value, &c.budget)
        });
        m.add_async_method(
            "send",
            |lua, c, (class, id, payload, timeout): (LV, u8, AnyUserData, Option<u64>)| async move {
                let class: MessageClass = lua.from_value(class)?;
                let bytes = payload.borrow::<Buffer>()?.bytes().to_vec();
                let timeout = timeout.unwrap_or(2000);
                if !(1..=120000).contains(&timeout) {
                    return Err(Fault::new(
                        "invalid_argument",
                        "gip.send",
                        "timeout_ms 1..120000 required",
                    )
                    .lua());
                }
                let result = tokio::time::timeout(Duration::from_millis(timeout), async {
                    let _gate = c.core.gate.lock().await;
                    c.core.send(class, id, bytes, timeout).await
                })
                .await
                .map_err(|_| {
                    Fault::new(
                        "timeout",
                        "gip.send",
                        "deadline exceeded; no automatic retry",
                    )
                    .lua()
                })?
                .map_err(Fault::lua)?;
                lua.to_value(&result)
            },
        );
        m.add_async_method("battery_info", |lua, c, timeout: Option<u64>| async move {
            lua.to_value(
                &c.core
                    .battery_info(timeout.unwrap_or(2000))
                    .await
                    .map_err(Fault::lua)?,
            )
        });
        m.add_async_method(
            "request",
            |lua,
             c,
             (class, id, payload, matcher, timeout): (
                LV,
                u8,
                AnyUserData,
                Function,
                Option<u64>,
            )| async move {
                let class: MessageClass = lua.from_value(class)?;
                let bytes = payload.borrow::<Buffer>()?.bytes().to_vec();
                let timeout = timeout.unwrap_or(2000);
                if !(1..=120000).contains(&timeout) {
                    return Err(Fault::new(
                        "invalid_argument",
                        "gip.request",
                        "timeout_ms 1..120000 required",
                    )
                    .lua());
                }
                c.core.authorize(class, id, &bytes).map_err(Fault::lua)?;
                tokio::time::timeout(Duration::from_millis(timeout), async {
                    let _gate = c.core.gate.lock().await;
                    let queue = c
                        .core
                        .subscribe(MessageFilter::default())
                        .map_err(Fault::lua)?;
                    struct Close(Arc<Queue>);
                    impl Drop for Close {
                        fn drop(&mut self) {
                            self.0.close();
                        }
                    }
                    let _close = Close(queue.clone());
                    c.core
                        .send(class, id, bytes, timeout)
                        .await
                        .map_err(Fault::lua)?;
                    let deadline = Instant::now() + Duration::from_millis(timeout);
                    loop {
                        c.core.ensure_open().map_err(Fault::lua)?;
                        let remaining = deadline
                            .saturating_duration_since(Instant::now())
                            .as_millis() as u64;
                        let value = queue.next(remaining).await.map_err(Fault::lua)?;
                        c.core.ensure_open().map_err(Fault::lua)?;
                        if value["kind"] == "timeout" {
                            return Err(Fault::new(
                                "timeout",
                                "gip.request",
                                "no matching response; no automatic retry",
                            )
                            .lua());
                        }
                        if value["kind"] == "overflow" {
                            return Err(Fault::new(
                                "overflow",
                                "gip.request",
                                "response stream overflowed",
                            )
                            .lua());
                        }
                        let reply = packet(&lua, value, &c.budget)?;
                        let result = scheduler::execute(
                            &lua,
                            matcher.clone(),
                            MultiValue::from_vec(vec![LV::Table(reply.clone())]),
                            remaining.clamp(1, 500),
                            100,
                            scheduler::current_cancel(),
                        )
                        .await?;
                        if result
                            .front()
                            .is_some_and(|v| !matches!(v, LV::Nil | LV::Boolean(false)))
                        {
                            return Ok(reply);
                        }
                    }
                })
                .await
                .map_err(|_| {
                    Fault::new(
                        "timeout",
                        "gip.request",
                        "deadline exceeded; native completion may still be pending",
                    )
                    .lua()
                })?
            },
        );
    }
}
