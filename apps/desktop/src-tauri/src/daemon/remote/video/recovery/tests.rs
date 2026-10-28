use super::super::{EncodedFrame, publish_frame};
use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};
#[derive(Default)]
struct Target {
    generation: AtomicU64,
    frames: AtomicUsize,
    pictures: Mutex<Vec<EncodedFrame>>,
    closed: AtomicBool,
}
impl VideoOut for Target {
    fn push(&self, frame: EncodedFrame, _: Instant) -> bool {
        self.pictures.lock().unwrap().push(frame);
        self.frames.fetch_add(1, Ordering::AcqRel);
        true
    }
    fn target_bitrate(&self) -> u64 {
        1_000_000
    }
    fn take_keyframe_request(&self) -> bool {
        false
    }
    fn closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
    fn reset(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
    fn video_epoch(&self) -> Option<u64> {
        Some(self.generation.load(Ordering::Acquire))
    }
    fn profile(&self) -> VideoProfile {
        VideoProfile::ConstrainedBaseline31
    }
}
struct Fixture {
    target: Arc<Target>,
    sender: Arc<Output>,
    stats: Arc<Stats>,
    resize: Resize,
    publication: Arc<Publication>,
}
impl Fixture {
    fn new() -> Self {
        let target = Arc::new(Target::default());
        Self {
            sender: Arc::new(Output::new(target.clone())),
            target,
            stats: Arc::new(Stats::default()),
            resize: Resize::new(640),
            publication: Arc::new(Publication::new(Arc::new(AtomicBool::new(false)))),
        }
    }
    fn context(&self) -> Context<'_> {
        Context {
            sender: &self.sender,
            stats: &self.stats,
            resize: &self.resize,
            publication: &self.publication,
        }
    }
    fn publish(&self, epoch: u64, keyframe: bool) {
        publish_frame(
            &self.stats,
            &self.publication,
            epoch,
            self.sender.as_ref(),
            frame(keyframe),
        );
    }
}
fn frame(keyframe: bool) -> EncodedFrame {
    EncodedFrame {
        cursor_embedded: None,
        data: vec![1],
        keyframe,
        captured: Instant::now(),
        latency: Duration::ZERO,
    }
}
fn opened<T>(stream: T) -> Opened<T> {
    Opened {
        stream,
        size: (640, 400),
        limit: 1280,
        profile: VideoProfile::ConstrainedBaseline31,
    }
}

#[test]
fn rebuilt_pipeline_requires_matching_decoder_readiness_and_an_accepted_idr() {
    let f = Fixture::new();
    let mut epoch = 0;
    let output = rebuild(
        (),
        "native device removed".into(),
        f.context(),
        || Ok(()),
        |_, generation| {
            epoch = generation;
            Ok(opened(()))
        },
    )
    .unwrap();
    assert_eq!(output.size, (640, 400));
    assert!(f.stats.recovery.active());
    assert_eq!(f.stats.recovery.describe()["state"], "warming");
    assert_eq!(f.resize.describe()["generation"], 1);
    assert!(f.sender.codec_ready());
    assert!(!f.sender.ready());
    f.publish(epoch, true);
    assert_eq!(f.target.frames.load(Ordering::Acquire), 0);
    assert!(f.stats.recovery.active());
    assert!(f.sender.activate(0).is_err());
    f.sender.activate(1).unwrap();
    f.publish(epoch, false);
    assert_eq!(f.target.frames.load(Ordering::Acquire), 0);
    f.publish(0, true);
    assert_eq!(
        f.target.frames.load(Ordering::Acquire),
        0,
        "retired callback escaped"
    );
    f.publish(epoch, true);
    assert_eq!(f.target.frames.load(Ordering::Acquire), 1);
    assert!(!f.stats.recovery.active());
    assert_eq!(f.stats.recovery.describe()["completed"], 1);
    assert_eq!(f.stats.keyframes.load(Ordering::Acquire), 1);
}

#[test]
fn publication_is_fenced_before_native_drop_can_deliver_a_late_frame() {
    struct Old {
        publication: Arc<Publication>,
        sender: Arc<Output>,
    }
    impl Drop for Old {
        fn drop(&mut self) {
            assert!(!self.publication.push_epoch(
                0,
                self.sender.as_ref(),
                frame(true),
                Instant::now()
            ));
        }
    }
    let f = Fixture::new();
    let old = Old {
        publication: f.publication.clone(),
        sender: f.sender.clone(),
    };
    let _ = rebuild(
        old,
        "capture ended".into(),
        f.context(),
        || Ok(()),
        |_, _| {
            Ok(opened(Old {
                publication: f.publication.clone(),
                sender: f.sender.clone(),
            }))
        },
    )
    .unwrap();
    assert_eq!(f.target.frames.load(Ordering::Acquire), 0);
}

#[test]
fn failed_initializations_get_distinct_epochs_and_cannot_publish_into_the_replacement() {
    let f = Fixture::new();
    let mut epochs = vec![];
    let _ = rebuild(
        (),
        "runtime fault".into(),
        f.context(),
        || Ok(()),
        |_, epoch| {
            epochs.push(epoch);
            if epochs.len() == 1 {
                Err("late initialization".into())
            } else {
                Ok(opened(()))
            }
        },
    )
    .unwrap();
    assert_eq!(epochs.len(), 2);
    assert_ne!(epochs[0], epochs[1]);
    f.sender.activate(2).unwrap();
    f.publish(epochs[0], true);
    assert!(f.stats.recovery.active());
    f.publish(epochs[1], true);
    assert!(!f.stats.recovery.active());
    assert_eq!(f.target.frames.load(Ordering::Acquire), 1);
}

#[test]
fn exhausted_runtime_rebuilds_stop_after_three_attempts() {
    let f = Fixture::new();
    let mut attempts = 0;
    let result = rebuild(
        (),
        "runtime fault".into(),
        f.context(),
        || Ok(()),
        |_, _| {
            attempts += 1;
            Err::<Opened<()>, _>("native still unavailable".into())
        },
    );
    assert!(result.err().unwrap().contains("停止自动重试"));
    assert_eq!(attempts, 3);
    assert_eq!(f.stats.recovery.describe()["attempts"], 3);
}

#[test]
fn authorization_loss_before_or_after_reopening_cannot_restart_publication() {
    for denied_before in [true, false] {
        let f = Fixture::new();
        let allowed = AtomicBool::new(!denied_before);
        let calls = AtomicUsize::new(0);
        let result = rebuild(
            (),
            "native stopped".into(),
            f.context(),
            || {
                if allowed.load(Ordering::Acquire) {
                    Ok(())
                } else {
                    Err("original grant ended".into())
                }
            },
            |_, _| {
                calls.fetch_add(1, Ordering::AcqRel);
                allowed.store(false, Ordering::Release);
                Ok(opened(()))
            },
        );
        assert_eq!(result.err().unwrap(), "original grant ended");
        assert_eq!(calls.load(Ordering::Acquire), usize::from(!denied_before));
        assert_eq!(f.target.frames.load(Ordering::Acquire), 0);
        assert!(!f.sender.ready() || denied_before);
    }
}

#[test]
fn retiring_a_lease_during_backoff_or_native_start_never_launches_extra_work() {
    for at_start in [true, false] {
        let f = Fixture::new();
        let calls = AtomicUsize::new(0);
        let result = rebuild(
            (),
            "native stopped".into(),
            f.context(),
            || {
                if at_start {
                    f.publication.close();
                }
                Ok(())
            },
            |_, _| {
                calls.fetch_add(1, Ordering::AcqRel);
                f.publication.close();
                Ok(opened(()))
            },
        );
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::Acquire), usize::from(!at_start));
        f.publish(f.publication.epoch.load(Ordering::Acquire), true);
        assert_eq!(f.target.frames.load(Ordering::Acquire), 0);
    }
}

#[test]
fn recovery_deadline_survives_repeated_warmup_failures_and_cannot_spawn_more_work() {
    let f = Fixture::new();
    f.stats.recovery.state().began = Instant::now().checked_sub(DEADLINE + Duration::from_secs(1));
    let result = rebuild(
        (),
        "never produced IDR".into(),
        f.context(),
        || Ok(()),
        |_, _| panic!("expired recovery reopened native state"),
    );
    assert!(result.err().unwrap().contains("停止自动重试"));
}

#[test]
fn successful_idrs_do_not_reset_the_recent_failure_budget() {
    let r = Recovery::default();
    let now = Instant::now();
    for _ in 0..ATTEMPTS {
        r.begin();
        r.attempt(now).unwrap();
        r.state().waiting = true;
        r.observed_idr();
    }
    r.begin();
    assert!(r.attempt(now + Duration::from_secs(1)).is_err());
    r.state().waiting = true;
    r.observed_idr();
    r.begin();
    assert!(
        r.attempt(now + WINDOW + Duration::from_millis(1)).is_err(),
        "an old recovery deadline must still expire"
    );
    r.state().began = Some(now + WINDOW);
    assert!(r.attempt(now + WINDOW + Duration::from_millis(1)).is_ok());
}

#[test]
#[ignore = "synthetic native codec acceptance; no screen, input or installed agent access"]
fn real_codec_rebuild_waits_for_ready_then_delivers_new_decodable_idr_and_geometry() {
    use dogcodes_remote_desktop::video::SyntheticEncoder;
    use openh264::formats::YUVSource;
    let f = Fixture::new();
    let sink = |epoch| {
        let (stats, publication, sender) =
            (f.stats.clone(), f.publication.clone(), f.sender.clone());
        Arc::new(move |frame| publish_frame(&stats, &publication, epoch, sender.as_ref(), frame))
            as dogcodes_remote_desktop::video::Sink
    };
    let mut original = SyntheticEncoder::with_profile(
        (640, 400),
        15,
        1_000_000,
        sink(0),
        VideoProfile::ConstrainedBaseline31,
    )
    .unwrap();
    original.encode_next(true).unwrap();
    assert_eq!(f.target.frames.load(Ordering::Acquire), 1);
    let mut opened = rebuild(
        original,
        "injected runtime device loss".into(),
        f.context(),
        || Ok(()),
        |_, epoch| {
            let stream = SyntheticEncoder::with_profile(
                (320, 200),
                15,
                1_000_000,
                sink(epoch),
                VideoProfile::ConstrainedBaseline31,
            )?;
            Ok(Opened {
                stream,
                size: (320, 200),
                limit: 1280,
                profile: VideoProfile::ConstrainedBaseline31,
            })
        },
    )
    .unwrap();
    opened.stream.encode_next(true).unwrap();
    assert_eq!(
        f.target.frames.load(Ordering::Acquire),
        1,
        "unready decoder received a replacement frame"
    );
    assert_eq!(f.resize.describe()["width"], 320);
    assert!(f.stats.recovery.active());
    f.sender.activate(1).unwrap();
    opened.stream.encode_next(true).unwrap();
    opened.stream.set_rate(500_000, 10);
    opened.stream.encode_next(false).unwrap();
    let pictures = f.target.pictures.lock().unwrap();
    assert_eq!(pictures.len(), 3);
    assert!(pictures[1].keyframe);
    let mut decoder = openh264::decoder::Decoder::new().unwrap();
    for frame in &pictures[1..] {
        assert_eq!(
            decoder.decode(&frame.data).unwrap().unwrap().dimensions(),
            (320, 200)
        );
    }
    assert!(!f.stats.recovery.active());
    assert_eq!(f.stats.recovery.describe()["completed"], 1);
}
