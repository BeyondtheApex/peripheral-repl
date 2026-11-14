mod userdata;
use super::{
    MessageClass,
    channel::{Backend, Core},
    feed::{Feed, Filter, MessageFilter},
    native,
};
use crate::{
    bindings::{self, Buffer, Context, Resource},
    error::Fault,
    queue::{Queue, Subscription},
    scheduler,
};
use mlua::{
    AnyUserData, Function, Lua, LuaSerdeExt, MultiValue, Table, UserData, UserDataMethods,
    Value as LV,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    rc::Rc,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use userdata::{Channel, Stream};

struct Watch(Arc<Queue>);
impl Resource for Watch {
    fn close(&self) {
        self.0.close()
    }
    fn closed(&self) -> bool {
        self.0.is_closed()
    }
}
fn channel(
    ctx: &Context,
    info: Value,
    backend: Backend,
    feed: Arc<Feed>,
    filter: MessageFilter,
) -> mlua::Result<Channel> {
    let core = Core::new(ctx, info, backend, feed, filter).map_err(Fault::lua)?;
    let channel = Channel {
        core,
        budget: ctx.budget.clone(),
    };
    ctx.manage(Rc::new(channel.clone()))?;
    Ok(channel)
}
pub fn install(lua: &Lua, ctx: &Context) -> mlua::Result<()> {
    // Materialize async userdata before the sandbox removes coroutine helpers.
    let dummy = channel(
        ctx,
        json!({"path":format!("gip:registration:{}",ctx.generation),"vendor_id":0,"product_id":0}),
        Backend::Mock(Mutex::new(VecDeque::new())),
        Arc::new(Feed::default()),
        MessageFilter::default(),
    )?;
    lua.create_userdata(dummy.clone())?;
    lua.create_userdata(Stream {
        channel: dummy.clone(),
        queue: dummy.core.input.clone(),
    })?;
    dummy.core.close();
    let gip = lua.create_table()?;
    gip.set(
        "capabilities",
        lua.create_function(|lua, ()| lua.to_value(&super::capabilities()))?,
    )?;
    gip.set(
        "diagnostics",
        lua.create_function(|lua, ()| lua.to_value(&super::diagnostics::snapshot()))?,
    )?;
    let c = ctx.clone();
    gip.set(
        "background",
        lua.create_async_function(move |_, ()| {
            let c = c.clone();
            async move { super::background::acquire(&c).await }
        })?,
    )?;
    gip.set(
        "list",
        lua.create_async_function(|lua, filter: Option<Table>| async move {
            let filter = filter
                .map(|v| lua.from_value(LV::Table(v)))
                .transpose()?
                .unwrap_or_default();
            lua.to_value(&super::list(filter).await.map_err(Fault::lua)?)
        })?,
    )?;
    let c = ctx.clone();
    gip.set(
        "open",
        lua.create_async_function(move |lua, (reference, filter): (Table, Option<Table>)| {
            let c = c.clone();
            async move {
                let reference = bindings::to_json(LV::Table(reference), 0)?;
                let filter = filter
                    .map(|v| lua.from_value(LV::Table(v)))
                    .transpose()?
                    .unwrap_or_default();
                let d = native::open(&reference).map_err(Fault::lua)?;
                let info = d.info.lock().unwrap().clone();
                channel(&c, info, Backend::Native(d.clone()), d.feed.clone(), filter)
            }
        })?,
    )?;
    let c = ctx.clone();
    gip.set("watch",lua.create_async_function(move|lua,filter:Option<Table>|{
        let c=c.clone();async move{
            let filter:Filter=filter.map(|v|lua.from_value(LV::Table(v))).transpose()?.unwrap_or_default();
            let initial=super::list(filter.clone()).await.map_err(Fault::lua)?;
            let q=Queue::new(64);c.manage(Rc::new(Watch(q.clone())))?;let output=q.clone();
            crate::bluetooth::executor().spawn(async move{
                let mut previous:BTreeMap<String,Value>=initial.into_iter().map(|v|(v["path"].as_str().unwrap().to_owned(),v)).collect();
                for d in previous.values(){output.push(json!({"kind":"present","device":d}));}output.push(json!({"kind":"ready"}));
                loop{
                    tokio::select!{_=output.cancellation.cancelled()=>break,_=tokio::time::sleep(Duration::from_millis(500))=>{}}
                    match super::snapshot(filter.clone()).await{
                        Ok(devices)=>{let now:BTreeMap<String,Value>=devices.into_iter().map(|v|(v["path"].as_str().unwrap().to_owned(),v)).collect();
                            for (path,d) in &now{if !previous.contains_key(path){output.push(json!({"kind":"added","device":d}));}}
                            for (path,d) in &previous{if !now.contains_key(path){output.push(json!({"kind":"removed","device":d}));}}
                            previous=now;
                        },Err(e)=>{output.push(json!({"kind":"error","error":e}));break;}
                    }
                }output.close();
            });Ok(Subscription(q))
        }
    })?)?;
    lua.globals().set("gip", gip)?;
    let c = ctx.clone();
    let mock: Table = lua.globals().get("mock")?;
    mock.set("gip",lua.create_function(move|lua,(steps,options):(Table,Option<Table>)|{
        let steps:Vec<Value>=lua.from_value(LV::Table(steps))?;
        super::replay::validate(&steps).map_err(Fault::lua)?;
        let options:super::replay::Options=options.map(|o|lua.from_value(LV::Table(o))).transpose()?.unwrap_or_default();
        let generation=uuid::Uuid::new_v4().to_string();
        channel(&c,json!({"backend":"gip_mock","path":format!("gip:mock:{generation}"),"generation":generation,"vendor_id":options.vendor_id,"product_id":options.product_id}),Backend::Mock(Mutex::new(steps.into())),Arc::new(Feed::default()),MessageFilter::default())
    })?)?;
    Ok(())
}
