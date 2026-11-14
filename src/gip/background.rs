//! Reference-counted, session-owned background delivery request.
use crate::bindings::{Context, Resource};
use mlua::{UserData, UserDataMethods};
#[cfg(windows)]
use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone)]
pub(super) struct Lease(Arc<State>);
struct State(AtomicBool);
impl State {
    fn close(&self) {
        if !self.0.swap(true, Ordering::AcqRel) {
            #[cfg(windows)]
            super::windows::background::release();
        }
    }
}
impl Drop for State {
    fn drop(&mut self) {
        self.close();
    }
}
impl Resource for Lease {
    fn close(&self) {
        self.0.close();
    }
    fn closed(&self) -> bool {
        self.0.0.load(Ordering::Acquire)
    }
}
impl UserData for Lease {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method("close", |_, s, ()| {
            s.close();
            Ok(())
        });
        m.add_method("active", |_, s, ()| Ok(!s.closed()));
    }
}
pub(super) async fn acquire(ctx: &Context) -> mlua::Result<Lease> {
    #[cfg(windows)]
    {
        // Return the RAII guard from the worker, so cancellation cannot leak a user.
        let lease = tokio::task::spawn_blocking(|| {
            super::windows::background::acquire()?;
            Ok::<_, crate::error::Fault>(Lease(Arc::new(State(AtomicBool::new(false)))))
        })
        .await
        .map_err(mlua::Error::external)?
        .map_err(crate::error::Fault::lua)?;
        ctx.manage(Rc::new(lease.clone()))?;
        Ok(lease)
    }
    #[cfg(not(windows))]
    {
        let _ = ctx;
        Err(crate::error::Fault::new(
            "unsupported",
            "gip.background",
            "Windows GameInput v4 required",
        )
        .lua())
    }
}
