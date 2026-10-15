//! Serial remote injection with an owned, expiring left-button lease.
//! Cancellation/renewal never queue behind movement. Only this worker injects
//! remote input; UI shutdown joins it only after removing low-level hooks.
use parking_lot::Mutex;
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};

const LEASE: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(50);
const RETRY: Duration = Duration::from_millis(100);
const CAPACITY: usize = 256;
const MAX_ID: u64 = 9_007_199_254_740_991;

pub enum Input {
    Move {
        dx: i32,
        dy: i32,
        drag_id: Option<u64>,
    },
    Scroll {
        dx: i32,
        dy: i32,
    },
    Zoom {
        delta: i32,
    },
    Tap {
        right: bool,
    },
    Key(String),
    Gesture(String),
    SwitchWindow {
        hwnd: isize,
    },
    SwitchDesktop {
        index: u32,
    },
}

struct Request {
    id: u64,
    active: bool,
    renewed: Instant,
}
struct Control {
    request: Mutex<Request>,
    cancelled: AtomicBool,
    generation_stop: Arc<AtomicBool>,
}
impl Control {
    fn new(generation_stop: Arc<AtomicBool>, now: Instant) -> Arc<Self> {
        Arc::new(Self {
            request: Mutex::new(Request {
                id: 0,
                active: false,
                renewed: now,
            }),
            cancelled: AtomicBool::new(false),
            generation_stop,
        })
    }
    fn stopped(&self) -> bool {
        self.cancelled.load(Ordering::Acquire) || self.generation_stop.load(Ordering::Acquire)
    }
    /// true = newly requested; false = duplicate still-live request.
    fn begin(&self, id: u64, now: Instant) -> Result<bool, &'static str> {
        if self.stopped() {
            return Err("stopped");
        }
        if !(1..=MAX_ID).contains(&id) {
            return Err("invalid_id");
        }
        let mut request = self.request.lock();
        if id == request.id && request.active {
            return Ok(false);
        }
        if id <= request.id {
            return Err("stale_id");
        }
        *request = Request {
            id,
            active: true,
            renewed: now,
        };
        Ok(true)
    }
    fn live(&self, id: u64, now: Instant) -> bool {
        if self.stopped() {
            return false;
        }
        let request = self.request.lock();
        request.id == id && request.active && now.saturating_duration_since(request.renewed) < LEASE
    }
    fn renew(&self, id: u64, now: Instant) -> bool {
        if self.stopped() {
            return false;
        }
        let mut request = self.request.lock();
        // A late heartbeat cannot resurrect an already-expired request.
        if request.id != id
            || !request.active
            || now.saturating_duration_since(request.renewed) >= LEASE
        {
            return false;
        }
        request.renewed = now;
        true
    }
    fn end(&self, id: u64) -> bool {
        let mut request = self.request.lock();
        if request.id != id || !request.active {
            return false;
        }
        request.active = false;
        true
    }
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

pub struct Session {
    control: Arc<Control>,
    writer: Arc<Mutex<TcpStream>>,
    sender: Option<SyncSender<Command>>,
}
impl Session {
    pub fn new(writer: Arc<Mutex<TcpStream>>, generation_stop: Arc<AtomicBool>) -> Arc<Self> {
        let sender = WORKER.lock().as_ref().map(|worker| worker.sender.clone());
        Arc::new(Self {
            control: Control::new(generation_stop, Instant::now()),
            writer,
            sender,
        })
    }
    pub fn begin_drag(self: &Arc<Self>, id: u64) -> Result<(), &'static str> {
        if !self.control.begin(id, Instant::now())? {
            return Ok(());
        }
        if let Err(error) = self.enqueue(Command::Begin {
            session: Arc::clone(self),
            id,
        }) {
            self.control.end(id);
            return Err(error);
        }
        Ok(())
    }
    pub fn renew_drag(&self, id: u64) -> bool {
        self.control.renew(id, Instant::now())
    }
    pub fn end_drag(&self, id: u64) -> bool {
        self.control.end(id)
    }
    pub fn cancel(&self) {
        self.control.cancel();
    }
    pub fn submit(self: &Arc<Self>, input: Input) -> Result<(), &'static str> {
        if self.control.stopped() {
            return Err("stopped");
        }
        self.enqueue(Command::Input {
            session: Arc::clone(self),
            input,
        })
    }
    fn enqueue(&self, command: Command) -> Result<(), &'static str> {
        if PROCESS_STOP.load(Ordering::Acquire) {
            return Err("stopped");
        }
        match self
            .sender
            .as_ref()
            .ok_or("worker_unavailable")?
            .try_send(command)
        {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err("busy"),
            Err(TrySendError::Disconnected(_)) => Err("worker_unavailable"),
        }
    }
    pub fn report_drag(&self, id: u64, state: &str, reason: Option<&str>) {
        let message = serde_json::json!({"t":"drag","id":id,"state":state,"reason":reason});
        self.report(&message);
    }
    fn report(&self, message: &serde_json::Value) {
        // No injection or control-state lock is held during a socket write.
        let result = crate::remote::write_frame(
            &mut self.writer.lock(),
            0x1,
            message.to_string().as_bytes(),
        );
        if result.is_err() {
            self.cancel();
        }
    }
}

enum Command {
    Begin { session: Arc<Session>, id: u64 },
    Input { session: Arc<Session>, input: Input },
}
struct Worker {
    sender: SyncSender<Command>,
    thread: JoinHandle<()>,
}
static WORKER: Mutex<Option<Worker>> = Mutex::new(None);
static PROCESS_STOP: AtomicBool = AtomicBool::new(false);

pub fn start() -> Result<(), String> {
    let mut worker = WORKER.lock();
    if PROCESS_STOP.load(Ordering::Acquire) {
        return Err("remote input is stopping".into());
    }
    if worker.is_some() {
        return Ok(());
    }
    let (sender, receiver) = mpsc::sync_channel(CAPACITY);
    let thread = std::thread::Builder::new()
        .name("remote-input".into())
        .spawn(move || run(receiver))
        .map_err(|error| format!("cannot start remote input: {error}"))?;
    *worker = Some(Worker { sender, thread });
    Ok(())
}

/// Process exit only. The caller MUST have uninstalled low-level hooks first.
/// Failed releases remain owed, even during shutdown; UIPI/secure-desktop
/// failures may delay exit until Windows accepts the release.
pub fn finish_shutdown() {
    PROCESS_STOP.store(true, Ordering::Release);
    let worker = WORKER.lock().take();
    if let Some(worker) = worker {
        drop(worker.sender);
        if worker.thread.join().is_err() {
            crate::log::write("remote input worker panicked");
        }
    }
}

trait Injector {
    fn left_down(&self) -> bool;
    fn button(&mut self, down: bool) -> bool;
    fn move_cursor(&mut self, dx: i32, dy: i32) -> bool;
    fn now(&self) -> Instant {
        Instant::now()
    }
}
struct Native;
impl Injector for Native {
    fn left_down(&self) -> bool {
        unsafe { GetAsyncKeyState(VK_LBUTTON as i32) < 0 }
    }
    fn button(&mut self, down: bool) -> bool {
        crate::win::hooks::send_remote_drag_button(down)
    }
    fn move_cursor(&mut self, dx: i32, dy: i32) -> bool {
        crate::scroll::injector::send_mouse_move(dx, dy)
    }
}

struct Held {
    control: Arc<Control>,
    id: Option<u64>, // None: checked left tap whose release still needs retry.
    debt: bool,
    warned: bool,
    next_retry: Instant,
}
#[derive(Debug, PartialEq)]
enum Notice {
    Ended(Option<u64>),
    ReleaseFailed(Option<u64>),
}
#[derive(Debug, PartialEq)]
enum Begin {
    Pressed,
    Cleaned(Notice),
    Rejected(&'static str),
}
#[derive(Default)]
struct Engine {
    held: Option<Held>,
}
impl Engine {
    fn tick(&mut self, sink: &mut impl Injector, stopping: bool) -> Option<Notice> {
        let now = sink.now();
        let held = self.held.as_mut()?;
        let live = held.id.is_some_and(|id| held.control.live(id, now));
        if !held.debt && (stopping || !live) {
            held.debt = true;
            held.next_retry = now;
            if let Some(id) = held.id {
                held.control.end(id);
            }
        }
        if !held.debt || now < held.next_retry {
            return None;
        }
        if sink.button(false) {
            let held = self.held.take().expect("held release");
            return Some(Notice::Ended(held.id));
        }
        held.next_retry = sink.now() + RETRY;
        if held.warned {
            return None;
        }
        held.warned = true;
        Some(Notice::ReleaseFailed(held.id))
    }
    fn begin(&mut self, control: Arc<Control>, id: u64, sink: &mut impl Injector) -> Begin {
        if !control.live(id, sink.now()) {
            control.end(id);
            return Begin::Rejected("cancelled_or_expired");
        }
        if let Some(held) = &self.held {
            if Arc::ptr_eq(&held.control, &control) && held.id == Some(id) && !held.debt {
                return Begin::Pressed; // A duplicate can acknowledge, never inject again.
            }
            control.end(id);
            return Begin::Rejected("busy");
        }
        if sink.left_down() {
            control.end(id);
            return Begin::Rejected("left_already_down");
        }
        if !sink.button(true) {
            control.end(id);
            return Begin::Rejected("input_unavailable");
        }
        self.held = Some(Held {
            control,
            id: Some(id),
            debt: false,
            warned: false,
            next_retry: sink.now(),
        });
        // Cancellation may have arrived while SendInput waited for the hook.
        match self.tick(sink, false) {
            Some(notice) => Begin::Cleaned(notice),
            None => Begin::Pressed,
        }
    }
    fn accepts(&self, control: &Arc<Control>, input: &Input, now: Instant) -> bool {
        if control.stopped() {
            return false;
        }
        match (&self.held, input) {
            (
                Some(held),
                Input::Move {
                    drag_id: Some(id), ..
                },
            ) => {
                !held.debt
                    && held.id == Some(*id)
                    && Arc::ptr_eq(&held.control, control)
                    && control.live(*id, now)
            }
            (Some(_), _)
            | (
                None,
                Input::Move {
                    drag_id: Some(_), ..
                },
            ) => false,
            (None, _) => !control.request.lock().active,
        }
    }
    fn failed_move(&mut self, sink: &mut impl Injector) -> Option<Notice> {
        if let Some(held) = self.held.as_mut() {
            held.debt = true;
            held.next_retry = sink.now();
            if let Some(id) = held.id {
                held.control.end(id);
            }
        }
        self.tick(sink, false)
    }
    fn left_tap(&mut self, control: Arc<Control>, sink: &mut impl Injector) -> Option<Notice> {
        if self.held.is_some() || control.stopped() || sink.left_down() || !sink.button(true) {
            return None;
        }
        self.held = Some(Held {
            control,
            id: None,
            debt: true,
            warned: false,
            next_retry: sink.now(),
        });
        self.tick(sink, false)
    }
}

fn report_notice(session: &Session, notice: Notice) {
    match notice {
        Notice::Ended(Some(id)) => session.report_drag(id, "ended", None),
        Notice::ReleaseFailed(id) => {
            crate::log::write("remote left-button release rejected; retaining release debt");
            if let Some(id) = id {
                session.report_drag(id, "release_failed", Some("input_unavailable"));
            } else {
                session.report(
                    &serde_json::json!({"t":"input_error","reason":"left_release_pending"}),
                );
            }
        }
        Notice::Ended(None) => {}
    }
}

fn run(receiver: Receiver<Command>) {
    let mut engine = Engine::default();
    let mut owner: Option<Arc<Session>> = None;
    let mut sink = Native;
    loop {
        let stopping = PROCESS_STOP.load(Ordering::Acquire);
        if let Some(notice) = engine.tick(&mut sink, stopping) {
            if let Some(session) = owner.as_ref() {
                report_notice(session, notice);
            }
        }
        if engine.held.is_none() {
            owner = None;
        }
        if stopping && owner.is_none() {
            return;
        }
        let command = match receiver.recv_timeout(POLL) {
            Ok(command) => command,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                PROCESS_STOP.store(true, Ordering::Release);
                if engine.held.is_some() {
                    std::thread::sleep(POLL);
                }
                continue;
            }
        };
        if PROCESS_STOP.load(Ordering::Acquire) {
            continue;
        }
        match command {
            Command::Begin { session, id } => {
                match engine.begin(Arc::clone(&session.control), id, &mut sink) {
                    Begin::Pressed => {
                        owner = Some(Arc::clone(&session));
                        session.report_drag(id, "started", None);
                    }
                    Begin::Cleaned(notice) => {
                        if engine.held.is_some() {
                            owner = Some(Arc::clone(&session));
                        }
                        report_notice(&session, notice);
                    }
                    Begin::Rejected(reason) => session.report_drag(id, "rejected", Some(reason)),
                }
            }
            Command::Input { session, input } => {
                if !engine.accepts(&session.control, &input, sink.now()) {
                    continue;
                }
                match input {
                    Input::Move { dx, dy, .. } => {
                        if !sink.move_cursor(dx, dy) {
                            if let Some(notice) = engine.failed_move(&mut sink) {
                                if let Some(session) = owner.as_ref() {
                                    report_notice(session, notice);
                                }
                            }
                        }
                    }
                    Input::Tap { right: false } => {
                        let notice = engine.left_tap(Arc::clone(&session.control), &mut sink);
                        if engine.held.is_some() {
                            owner = Some(Arc::clone(&session));
                        }
                        if let Some(notice) = notice {
                            report_notice(&session, notice);
                        }
                    }
                    input => ordinary_input(input, &session),
                }
            }
        }
    }
}

fn ordinary_input(input: Input, session: &Session) {
    match input {
        Input::Scroll { dx, dy } => {
            if dy != 0 {
                crate::win::hooks::push_remote_scroll(dy, false);
            }
            if dx != 0 {
                crate::win::hooks::push_remote_scroll(dx, true);
            }
        }
        Input::Zoom { delta } => {
            if delta != 0 {
                crate::win::hooks::send_remote_zoom(delta);
            }
        }
        Input::Tap { right: true } => crate::win::hooks::send_remote_click(true),
        Input::Key(text) => crate::win::hooks::send_remote_text(&text),
        Input::Gesture(gesture) => crate::win::hooks::send_remote_gesture(&gesture),
        Input::SwitchWindow { hwnd } => {
            let ok = crate::win::window_list::focus_window(hwnd);
            session.report(&serde_json::json!({"t":"switch_result","ok":ok}));
        }
        Input::SwitchDesktop { index } => crate::win::window_list::switch_to_desktop(index),
        Input::Move { .. } | Input::Tap { right: false } => {
            unreachable!("owned input handled above")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Debug, PartialEq)]
    enum Event {
        Down,
        Up,
        Move(i32, i32),
    }
    struct Fake {
        clock: Instant,
        down: bool,
        reject_down: bool,
        reject_up: bool,
        reject_move: bool,
        cancel_on_down: Option<Arc<Control>>,
        events: Vec<Event>,
    }
    impl Default for Fake {
        fn default() -> Self {
            Self {
                clock: Instant::now(),
                down: false,
                reject_down: false,
                reject_up: false,
                reject_move: false,
                cancel_on_down: None,
                events: Vec::new(),
            }
        }
    }
    impl Injector for Fake {
        fn left_down(&self) -> bool {
            self.down
        }
        fn button(&mut self, down: bool) -> bool {
            if (down && self.reject_down) || (!down && self.reject_up) {
                return false;
            }
            self.down = down;
            self.events.push(if down { Event::Down } else { Event::Up });
            if down {
                if let Some(control) = self.cancel_on_down.take() {
                    control.cancel();
                }
            }
            true
        }
        fn move_cursor(&mut self, dx: i32, dy: i32) -> bool {
            if self.reject_move {
                return false;
            }
            self.events.push(Event::Move(dx, dy));
            true
        }
        fn now(&self) -> Instant {
            self.clock
        }
    }
    fn control(sink: &Fake) -> Arc<Control> {
        Control::new(Arc::new(AtomicBool::new(false)), sink.now())
    }
    fn begin(engine: &mut Engine, control: &Arc<Control>, id: u64, sink: &mut Fake) -> Begin {
        control.begin(id, sink.now()).unwrap();
        engine.begin(Arc::clone(control), id, sink)
    }
    #[test]
    fn duplicate_and_retired_ids_cannot_press_twice() {
        let mut sink = Fake::default();
        let control = control(&sink);
        let mut engine = Engine::default();
        assert_eq!(begin(&mut engine, &control, 1, &mut sink), Begin::Pressed);
        assert_eq!(control.begin(1, sink.now()), Ok(false));
        assert_eq!(
            engine.begin(Arc::clone(&control), 1, &mut sink),
            Begin::Pressed
        );
        assert_eq!(sink.events, vec![Event::Down]);
        assert!(control.end(1));
        assert_eq!(engine.tick(&mut sink, false), Some(Notice::Ended(Some(1))));
        assert!(!control.end(1));
        assert_eq!(control.begin(1, sink.now()), Err("stale_id"));
        assert_eq!(sink.events, vec![Event::Down, Event::Up]);
    }
    #[test]
    fn foreign_and_stale_commands_cannot_touch_the_owner() {
        let mut sink = Fake::default();
        let owner = control(&sink);
        let foreign = control(&sink);
        let mut engine = Engine::default();
        assert_eq!(begin(&mut engine, &owner, 3, &mut sink), Begin::Pressed);
        // A queued foreign start is live before the worker rejects it. A move
        // queued before that start must still never borrow the owner's button.
        foreign.begin(3, sink.now()).unwrap();
        assert!(!engine.accepts(
            &foreign,
            &Input::Move {
                dx: 5,
                dy: 0,
                drag_id: Some(3)
            },
            sink.now()
        ));
        assert_eq!(
            begin(&mut engine, &foreign, 3, &mut sink),
            Begin::Rejected("busy")
        );
        assert!(!foreign.end(3));
        assert!(!foreign.renew(3, sink.now()));
        assert!(!owner.end(2));
        assert!(!owner.renew(2, sink.now()));
        for control in [&owner, &foreign] {
            for input in [
                Input::Move {
                    dx: 5,
                    dy: 0,
                    drag_id: None,
                },
                Input::Tap { right: false },
                Input::Key("no".into()),
                Input::Scroll { dx: 0, dy: 120 },
                Input::SwitchWindow { hwnd: 9 },
            ] {
                assert!(!engine.accepts(control, &input, sink.now()));
            }
        }
        assert!(!engine.accepts(
            &foreign,
            &Input::Move {
                dx: 5,
                dy: 0,
                drag_id: Some(3)
            },
            sink.now()
        ));
        assert!(!engine.accepts(
            &owner,
            &Input::Move {
                dx: 5,
                dy: 0,
                drag_id: Some(2)
            },
            sink.now()
        ));
        assert!(engine.accepts(
            &owner,
            &Input::Move {
                dx: 5,
                dy: 0,
                drag_id: Some(3)
            },
            sink.now()
        ));
        assert_eq!(engine.tick(&mut sink, false), None);
        assert_eq!(sink.events, vec![Event::Down]);
    }
    #[test]
    fn physical_or_failed_down_never_creates_release_debt() {
        for physical in [true, false] {
            let mut sink = Fake {
                down: physical,
                reject_down: !physical,
                ..Fake::default()
            };
            let control = control(&sink);
            let mut engine = Engine::default();
            let reason = if physical {
                "left_already_down"
            } else {
                "input_unavailable"
            };
            assert_eq!(
                begin(&mut engine, &control, 1, &mut sink),
                Begin::Rejected(reason)
            );
            control.cancel();
            assert_eq!(engine.tick(&mut sink, true), None);
            assert!(engine.held.is_none());
            assert!(sink.events.is_empty());
            assert_eq!(sink.down, physical);
        }
    }
    #[test]
    fn failed_up_is_retained_until_success_and_cannot_be_renewed_away() {
        let mut sink = Fake::default();
        let owner = control(&sink);
        let foreign = control(&sink);
        let mut engine = Engine::default();
        assert_eq!(begin(&mut engine, &owner, 1, &mut sink), Begin::Pressed);
        owner.end(1);
        sink.reject_up = true;
        assert_eq!(
            engine.tick(&mut sink, false),
            Some(Notice::ReleaseFailed(Some(1)))
        );
        assert!(!owner.renew(1, sink.now()));
        assert_eq!(
            begin(&mut engine, &owner, 2, &mut sink),
            Begin::Rejected("busy")
        );
        assert_eq!(
            begin(&mut engine, &foreign, 1, &mut sink),
            Begin::Rejected("busy")
        );
        assert!(sink.down);
        assert_eq!(engine.tick(&mut sink, true), None);
        sink.clock += RETRY;
        assert_eq!(engine.tick(&mut sink, true), None);
        assert!(engine.held.is_some());
        sink.reject_up = false;
        sink.clock += RETRY;
        assert_eq!(engine.tick(&mut sink, true), Some(Notice::Ended(Some(1))));
        assert!(!sink.down);
        assert!(engine.held.is_none());
        assert_eq!(sink.events, vec![Event::Down, Event::Up]);
    }
    #[test]
    fn heartbeat_keeps_stationary_drag_but_late_renew_does_not_resurrect_it() {
        let mut sink = Fake::default();
        let control = control(&sink);
        let mut engine = Engine::default();
        assert_eq!(begin(&mut engine, &control, 1, &mut sink), Begin::Pressed);
        for _ in 0..20 {
            sink.clock += Duration::from_millis(250);
            assert!(control.renew(1, sink.now()));
            assert_eq!(engine.tick(&mut sink, false), None);
        }
        sink.clock += LEASE;
        assert!(!control.renew(1, sink.now()));
        assert_eq!(engine.tick(&mut sink, false), Some(Notice::Ended(Some(1))));
        assert!(!sink.down);
        assert_eq!(control.begin(1, sink.now()), Err("stale_id"));
    }
    #[test]
    fn cancelled_and_expired_queued_starts_never_inject_down() {
        for cancel in [true, false] {
            let mut sink = Fake::default();
            let control = control(&sink);
            let mut engine = Engine::default();
            control.begin(1, sink.now()).unwrap();
            if cancel {
                control.cancel();
            } else {
                sink.clock += LEASE;
            }
            assert_eq!(
                engine.begin(control, 1, &mut sink),
                Begin::Rejected("cancelled_or_expired")
            );
            assert!(sink.events.is_empty());
        }
    }
    #[test]
    fn cancellation_during_down_and_generation_stop_order_release_after_down() {
        let mut sink = Fake::default();
        let control = control(&sink);
        let mut engine = Engine::default();
        sink.cancel_on_down = Some(Arc::clone(&control));
        assert_eq!(
            begin(&mut engine, &control, 1, &mut sink),
            Begin::Cleaned(Notice::Ended(Some(1)))
        );
        assert_eq!(sink.events, vec![Event::Down, Event::Up]);
        assert!(engine.held.is_none());
        let next = super::Control::new(Arc::new(AtomicBool::new(false)), sink.now());
        assert_eq!(begin(&mut engine, &next, 1, &mut sink), Begin::Pressed);
        next.generation_stop.store(true, Ordering::Release);
        assert_eq!(engine.tick(&mut sink, false), Some(Notice::Ended(Some(1))));
        assert!(!sink.down);
    }
    #[test]
    fn saturated_queue_cannot_drop_end_or_cancel() {
        use std::net::TcpListener;
        for cancel in [false, true] {
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let writer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            let (_peer, _) = listener.accept().unwrap();
            let (tx, rx) = mpsc::sync_channel(1);
            let mut sink = Fake::default();
            let session = Arc::new(Session {
                control: control(&sink),
                writer: Arc::new(Mutex::new(writer)),
                sender: Some(tx),
            });
            let mut engine = Engine::default();
            session.begin_drag(1).unwrap();
            let Command::Begin { id, .. } = rx.recv().unwrap() else {
                panic!("expected begin");
            };
            assert_eq!(
                engine.begin(Arc::clone(&session.control), id, &mut sink),
                Begin::Pressed
            );
            session
                .submit(Input::Move {
                    dx: 20,
                    dy: 0,
                    drag_id: Some(1),
                })
                .unwrap();
            assert_eq!(
                session.submit(Input::Move {
                    dx: 20,
                    dy: 0,
                    drag_id: Some(1)
                }),
                Err("busy")
            );
            if cancel {
                session.cancel();
            } else {
                assert!(session.end_drag(1));
            }
            assert_eq!(engine.tick(&mut sink, false), Some(Notice::Ended(Some(1))));
            assert!(!sink.down);
            let Command::Input { input, .. } = rx.recv().unwrap() else {
                panic!("expected queued move");
            };
            assert!(!engine.accepts(&session.control, &input, sink.now()));
            assert_eq!(sink.events, vec![Event::Down, Event::Up]);
        }
    }
    #[test]
    fn move_failure_and_partial_tap_retain_release_debt() {
        let mut sink = Fake::default();
        let control = control(&sink);
        let mut engine = Engine::default();
        assert_eq!(begin(&mut engine, &control, 1, &mut sink), Begin::Pressed);
        sink.reject_move = true;
        sink.reject_up = true;
        assert!(!sink.move_cursor(5, 0));
        assert_eq!(
            engine.failed_move(&mut sink),
            Some(Notice::ReleaseFailed(Some(1)))
        );
        assert!(sink.down);
        sink.reject_up = false;
        sink.clock += RETRY;
        engine.tick(&mut sink, false);
        assert!(!sink.down);
        let next = super::Control::new(Arc::new(AtomicBool::new(false)), sink.now());
        sink.reject_up = true;
        assert_eq!(
            engine.left_tap(next, &mut sink),
            Some(Notice::ReleaseFailed(None))
        );
        assert!(sink.down);
        sink.reject_up = false;
        sink.clock += RETRY;
        assert_eq!(engine.tick(&mut sink, true), Some(Notice::Ended(None)));
        assert!(!sink.down);
    }
    #[test]
    fn invalid_ids_and_old_moves_do_not_reactivate_finished_drag() {
        let mut sink = Fake::default();
        let control = control(&sink);
        let mut engine = Engine::default();
        for id in [0, MAX_ID + 1, u64::MAX] {
            assert_eq!(control.begin(id, sink.now()), Err("invalid_id"));
        }
        assert_eq!(
            begin(&mut engine, &control, MAX_ID, &mut sink),
            Begin::Pressed
        );
        control.end(MAX_ID);
        engine.tick(&mut sink, false);
        assert!(!engine.accepts(
            &control,
            &Input::Move {
                dx: 20,
                dy: 0,
                drag_id: Some(MAX_ID)
            },
            sink.now()
        ));
        assert!(engine.accepts(
            &control,
            &Input::Move {
                dx: 20,
                dy: 0,
                drag_id: None
            },
            sink.now()
        ));
    }
    #[test]
    fn enumerated_sequences_preserve_single_press_and_eventual_release() {
        // 7^4 deterministic adversarial sequences: duplicate/foreign starts,
        // end, renewal, expiry and cancellation. No external clock or globals.
        for sequence in 0..2401usize {
            let mut code = sequence;
            let mut sink = Fake::default();
            let mut engine = Engine::default();
            let a = control(&sink);
            let b = control(&sink);
            for _ in 0..4 {
                engine.tick(&mut sink, false);
                match code % 7 {
                    0 | 1 => {
                        let c = if code % 7 == 0 { &a } else { &b };
                        if c.begin(1, sink.now()).is_ok() {
                            engine.begin(Arc::clone(c), 1, &mut sink);
                        }
                    }
                    2 => {
                        a.end(1);
                    }
                    3 => {
                        b.end(1);
                    }
                    4 => {
                        a.renew(1, sink.now());
                    }
                    5 => {
                        sink.clock += LEASE;
                    }
                    _ => a.cancel(),
                }
                code /= 7;
                engine.tick(&mut sink, false);
                let mut pressed = false;
                for event in &sink.events {
                    match event {
                        Event::Down => {
                            assert!(!pressed);
                            pressed = true;
                        }
                        Event::Up => {
                            assert!(pressed);
                            pressed = false;
                        }
                        Event::Move(_, _) => {}
                    }
                }
                assert_eq!(pressed, sink.down);
            }
            engine.tick(&mut sink, true);
            assert!(
                !sink.down,
                "sequence {sequence} did not release on shutdown"
            );
            assert!(engine.held.is_none());
        }
    }
}
