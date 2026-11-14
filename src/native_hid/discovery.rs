//! One discovery worker per host process. Device channels retain their own
//! isolation; discovery never opens a device and cannot head-of-line block I/O.
use super::*;

#[derive(Default)]
pub(super) struct Discovery {
    worker: Option<NativeHid>,
}
impl Discovery {
    pub fn enumerate(&mut self, filter: Option<&Value>) -> Result<Value, Fault> {
        self.enumerate_with(filter, NativeHid::spawn)
    }
    fn enumerate_with(
        &mut self,
        filter: Option<&Value>,
        spawn: impl FnOnce() -> Result<NativeHid, Fault>,
    ) -> Result<Value, Fault> {
        if self.worker.is_none() {
            self.worker = Some(spawn()?);
        }
        let worker = self.worker.as_ref().unwrap();
        // No cache: each request observes fresh hotplug state and its own filter.
        let result = worker.call(json!({"op":"enumerate","filter":filter}));
        if worker.process.lock().unwrap().dead {
            self.worker.take();
        }
        // A failed/dead worker is replaced on the next request. Never retry an
        // unbounded number of times inside one call or share late responses.
        result
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    fn discovery_reuses_one_process_and_preserves_fresh_filters() {
        let mut discovery = Discovery::default();
        for n in 0..8 {
            let result = discovery.enumerate_with(Some(&json!({"vendor_id":n})), || {
                assert_eq!(n, 0, "repeated enumeration spawned another process");
                Ok(super::super::tests::ready_worker("[Console]::WriteLine('{\"ready\":true}'); $n=0; while($l=[Console]::ReadLine()){ $r=$l|ConvertFrom-Json; $n++; @{ok=$true;result=@{sequence=$n;filter=$r.filter}}|ConvertTo-Json -Compress -Depth 5|%{[Console]::WriteLine($_)} }"))
            }).unwrap();
            assert_eq!(result["sequence"], n + 1);
            assert_eq!(result["filter"]["vendor_id"], n);
        }
    }
    #[test]
    fn failed_discovery_is_replaced_without_consuming_a_late_reply() {
        let mut discovery = Discovery::default();
        let result = discovery.enumerate_with(None, || Ok(super::super::tests::ready_worker(
            "[Console]::WriteLine('{\"ready\":true}'); [Console]::ReadLine()|Out-Null; Start-Sleep -Seconds 30"
        )));
        assert_eq!(result.unwrap_err().code, "disconnected");
        assert!(discovery.worker.is_none());
        let result=discovery.enumerate_with(None, || Ok(super::super::tests::ready_worker(
            "[Console]::WriteLine('{\"ready\":true}'); [Console]::ReadLine()|Out-Null; [Console]::WriteLine('{\"ok\":true,\"result\":[{\"path\":\"recovered\"}]}')"
        ))).unwrap();
        assert_eq!(result[0]["path"], "recovered");
    }
}
