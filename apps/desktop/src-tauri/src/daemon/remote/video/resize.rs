//! Latest desired resolution, scoped to one video instance. No OS calls on
//! the RPC/input worker; starting/tearing down codecs stays on the video worker.
use super::{Duration, Instant, Mutex, Value, json};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Request {
    pub seq: u64,
    pub width: u32,
}
struct State {
    desired: Request,
    applied: Request,
    size: (u32, u32),
    limit: u32,
    working: Option<Instant>,
    generation: Option<u64>,
}
pub(super) struct Resize {
    id: String,
    state: Mutex<State>,
}
impl Resize {
    pub fn new(width: u32) -> Self {
        let request = Request { seq: 0, width };
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            state: Mutex::new(State {
                desired: request,
                applied: request,
                size: (0, 0),
                limit: width,
                working: None,
                generation: None,
            }),
        }
    }
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub fn request(&self, instance: &str, seq: u64, width: u32) -> Result<(), String> {
        if !(320..=3840).contains(&width) {
            return Err("视频尺寸超出支持范围".into());
        }
        // A late request from the previous monitor cannot resize its successor.
        if instance != self.id {
            return Ok(());
        }
        let mut state = self.state();
        if seq < state.desired.seq {
            return Ok(());
        }
        if seq == state.desired.seq {
            return if width == state.desired.width {
                Ok(())
            } else {
                Err("视频清晰度请求身份冲突".into())
            };
        }
        state.desired = Request { seq, width };
        Ok(())
    }
    pub fn pending(&self) -> Option<Request> {
        let s = self.state();
        (s.desired.seq > s.applied.seq).then_some(s.desired)
    }
    pub fn begin(&self) {
        self.state().working = Some(Instant::now());
    }
    pub fn applied(&self, request: Request, size: (u32, u32), limit: u32, generation: Option<u64>) {
        let mut s = self.state();
        s.applied = request;
        s.size = size;
        s.limit = limit.max(320);
        s.generation = generation;
        s.working = None;
    }
    pub fn latest(&self) -> Request {
        self.state().desired
    }
    pub fn target(&self) -> u32 {
        self.state().desired.width
    }
    pub fn describe(&self) -> Value {
        let s = self.state();
        let mut value = json!({"codec":"h264","width":s.size.0,"height":s.size.1,
          "resize":{"instanceId":self.id,"requested":s.desired.seq.to_string(),"applied":s.applied.seq.to_string(),
            "targetWidth":s.applied.width,"maxWidth":s.limit,"state":if s.working.is_some(){"updating"}else if s.desired.seq>s.applied.seq{"pending"}else{"ready"}}});
        if let Some(generation) = s.generation {
            value["generation"] = json!(generation);
        }
        value
    }
    pub fn migration_ready(&self) -> bool {
        let s = self.state();
        s.working.is_none() && s.desired.seq == s.applied.seq
    }
    pub fn migrated(&self, generation: u64) {
        self.state().generation = Some(generation);
    }
    pub fn timeout(&self) -> Option<String> {
        self.state()
            .working
            .filter(|at| at.elapsed() > Duration::from_secs(10))
            .map(|_| "视频清晰度调整超时".into())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn geometry_keeps_its_published_generation_during_native_work_and_times_out() {
        let board = Resize::new(640);
        board.applied(Request { seq: 0, width: 640 }, (640, 400), 1280, Some(7));
        board.request(&board.id, 1, 960).unwrap();
        board.begin();
        assert_eq!(board.describe()["generation"], 7);
        assert_eq!(board.describe()["width"], 640);
        assert_eq!(board.describe()["resize"]["state"], "updating");
        board.state().working = Instant::now().checked_sub(Duration::from_secs(11));
        assert!(board.timeout().is_some());
        board.applied(Request { seq: 1, width: 960 }, (960, 600), 1280, Some(8));
        assert!(board.timeout().is_none());
        assert_eq!(board.describe()["generation"], 8);
        assert_eq!(board.describe()["width"], 960);
    }
    #[test]
    fn queued_resolution_is_latest_idempotent_and_cannot_cross_monitor_instances() {
        let board = Resize::new(640);
        let id = board.id.clone();
        board.applied(Request { seq: 0, width: 640 }, (640, 400), 1280, None);
        board.request(&id, 1, 960).unwrap();
        board.begin();
        board.request(&id, 3, 1280).unwrap();
        board.request(&id, 2, 800).unwrap();
        board.applied(Request { seq: 1, width: 960 }, (960, 600), 1280, None);
        assert_eq!(
            board.pending(),
            Some(Request {
                seq: 3,
                width: 1280
            })
        );
        board.request(&id, 3, 1280).unwrap();
        assert!(board.request(&id, 3, 960).is_err());
        board.request("old-monitor", 999, 640).unwrap();
        assert_eq!(board.target(), 1280);
        assert!(board.request(&id, 4, 100_000).is_err());
        board.applied(
            Request {
                seq: 3,
                width: 1280,
            },
            (1280, 800),
            1280,
            None,
        );
        assert_eq!(board.pending(), None);
        assert_eq!(board.describe()["resize"]["state"], "ready");
    }
}
