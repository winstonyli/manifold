use std::sync::{Arc, Mutex};

use super::*;

type Slot = Arc<Mutex<Option<mpsc::Receiver<HostToView>>>>;
type Seen = Arc<Mutex<Vec<Option<HostToView>>>>;

/// A wake closure that, at each wake, takes one message from the inbox and records what it found: a wake that
/// comes before its message, a second wake for one message, or a wake while the slot is still empty records `None`.
/// The slot exists because `spawn_host_reader` returns the receiver after taking the closure.
fn recording_wake() -> (Slot, Seen, impl Fn() + Send + 'static) {
    let (slot, seen): (Slot, Seen) = Default::default();
    let (s, r) = (slot.clone(), seen.clone());
    let wake = move || {
        let got = s.lock().unwrap().as_ref().and_then(|rx| rx.try_recv().ok());
        r.lock().unwrap().push(got);
    };
    (slot, seen, wake)
}

/// Waits (up to 5 s) for `n` wakes, then until the reader has exited (its sender is gone, so no later wake can come),
/// and returns every wake recorded. A message still in the inbox then is one no wake took: it fails here.
fn wakes_after_exit(slot: &Slot, seen: &Seen, n: usize) -> Vec<Option<HostToView>> {
    let end = Instant::now() + Duration::from_secs(5);
    while seen.lock().unwrap().len() < n {
        assert!(Instant::now() < end, "only {:?} after 5 s", seen.lock().unwrap());
        std::thread::sleep(Duration::from_millis(2));
    }
    loop {
        match slot.lock().unwrap().as_ref().unwrap().try_recv() {
            Err(mpsc::TryRecvError::Disconnected) => break,
            Err(mpsc::TryRecvError::Empty) => {}
            Ok(msg) => panic!("{msg:?} arrived with no wake to take it"),
        }
        assert!(Instant::now() < end, "the reader did not exit within 5 s");
        std::thread::sleep(Duration::from_millis(2));
    }
    seen.lock().unwrap().clone()
}

/// A reader on one end of a local TCP pair, with its receiver in `slot` before the host end (`host`) writes anything.
fn reader_then_host(slot: &Slot, wake: impl Fn() + Send + 'static, host: impl FnOnce(TcpStream) + Send + 'static) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let view_end = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (host_end, _) = listener.accept().unwrap();
    *slot.lock().unwrap() = Some(spawn_host_reader(MessageChannel::new(view_end), wake));
    std::thread::spawn(move || host(host_end)).join().unwrap();
}

#[test]
fn every_wake_finds_its_message() {
    let (slot, seen, wake) = recording_wake();
    reader_then_host(&slot, wake, |stream| {
        let mut out = MessageChannel::new(stream);
        out.send_version().unwrap();
        for _ in 0..3 {
            out.send(&HostToView::GetState).unwrap();
        }
        // dropped: the reader adds its own Shutdown
    });
    let seen = wakes_after_exit(&slot, &seen, 4);
    assert_eq!(seen.len(), 4, "{seen:?}");
    assert!(seen[..3].iter().all(Option::is_some), "{seen:?}");
    assert!(matches!(seen[3], Some(HostToView::Shutdown)), "{seen:?}");
}

#[test]
fn a_host_gone_before_its_version_wakes() {
    let (slot, seen, wake) = recording_wake();
    reader_then_host(&slot, wake, drop);
    let seen = wakes_after_exit(&slot, &seen, 1);
    assert!(matches!(seen[..], [Some(HostToView::Shutdown)]), "{seen:?}");
}

/// Pins the WebKit-view-host hang fix: `accept_remote` must not call
/// `TcpListener::accept()` unbounded, which blocks indefinitely if
/// nobody ever connects. Spawns run on a worker thread, but an
/// unbounded wait would still strand the tab and its launcher (on the
/// UI thread it would freeze the *entire app* for as long as a
/// WSL2-hosted view-host took to dial back, which can reach hours).
/// This doesn't reproduce a cold WSL2 boot (out of scope for a unit
/// test), but it does pin the actual behavioral contract: given nobody ever
/// connects at all, `accept_remote` must return within (not much
/// more than) its `timeout` argument, as a clean `TimedOut` error, not
/// hang — the exact shape of the bug that let the freeze happen.
#[test]
fn a_peer_dripping_its_token_is_cut_off_at_the_limit() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral port");
    let port = listener.local_addr().expect("addr").port();
    let peer = std::thread::spawn(move || {
        let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        for _ in 0..TOKEN_LEN {
            if s.write_all(&[7]).is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(400));
        }
    });
    let (mut stream, _) = listener.accept().expect("accept");
    let start = Instant::now();
    assert_eq!(read_token(&mut stream, Instant::now() + Duration::from_secs(30)), None);
    assert!(start.elapsed() < Duration::from_millis(1500), "bounded by the one second, not by each byte");
    drop(stream);
    peer.join().expect("peer");
}

#[test]
fn accept_remote_times_out_when_nobody_connects() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral port");
    let timeout = Duration::from_millis(200);

    let start = Instant::now();
    let result = accept_remote(&listener, timeout, &[7; TOKEN_LEN]);
    let elapsed = start.elapsed();

    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("accept_remote should time out, not succeed"),
    };
    assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    // Generous upper bound (not a tight timing assertion) — this just
    // needs to prove it didn't block indefinitely, not measure exact
    // poll-loop overhead.
    assert!(elapsed < timeout * 5, "accept_remote took {elapsed:?}, expected close to the {timeout:?} timeout");
}

/// Same regression, for `accept_local_pair`'s bounded wait (added
/// alongside the WebKit fix, hardening the same unbounded-`accept()`
/// pattern on the same-machine path) — a listener with no
/// client ever connecting must time out cleanly rather than hang.
#[test]
fn accept_local_pair_times_out_when_nobody_connects() {
    let pair = listen_local_pair("test-accept-local-pair-timeout").expect("listen_local_pair");
    let timeout = Duration::from_millis(200);

    let start = Instant::now();
    let result = accept_local_pair(&pair, timeout);
    let elapsed = start.elapsed();

    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("accept_local_pair should time out, not succeed"),
    };
    assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    // Two sequential per-socket timeouts (see accept_local_pair's doc
    // comment) — worst case is up to ~2x timeout, not 1x.
    assert!(elapsed < timeout * 10, "accept_local_pair took {elapsed:?}, expected roughly 2x the {timeout:?} timeout");
}

#[test]
fn a_dropped_host_becomes_shutdown() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let host = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.write_all(&crate::PROTOCOL_VERSION.to_le_bytes()).unwrap();
        // dropped: the engine's reader must turn this into Shutdown
    });
    let rx = spawn_host_reader(MessageChannel::new(TcpStream::connect(("127.0.0.1", port)).unwrap()), || {});
    host.join().unwrap();
    assert!(matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), HostToView::Shutdown));
}

#[test]
fn a_wrong_version_from_the_host_becomes_shutdown() {
    let (slot, seen, wake) = recording_wake();
    reader_then_host(&slot, wake, |mut stream| {
        stream.write_all(&(crate::PROTOCOL_VERSION + 1).to_le_bytes()).unwrap();
        // A well-formed Navigate after it must never be delivered. Best
        // effort: the reader may already have hung up on the bad version.
        let _ = MessageChannel::new(stream).send(&HostToView::Navigate { url: "https://x".into(), referrer: None });
    });
    let seen = wakes_after_exit(&slot, &seen, 1);
    assert!(matches!(seen[..], [Some(HostToView::Shutdown)]), "{seen:?}");
}

/// The host speaks `PROTOCOL_VERSION`; manifold's number decides which side the host's log line says to rebuild.
#[test]
fn the_host_names_which_side_to_rebuild() {
    let v = crate::PROTOCOL_VERSION;
    assert_eq!(skew_with_app(v), None);
    assert!(skew_with_app(v + 1).unwrap().to_string().ends_with("; rebuild the view host"));
    assert!(skew_with_app(v - 1).unwrap().to_string().ends_with("; rebuild manifold"));
}

/// An intruder that keeps reconnecting with a bad token must not keep
/// `accept_remote` past its deadline.
#[test]
fn accept_remote_times_out_under_a_stream_of_bad_peers() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let intruder = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                if let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) {
                    let _ = s.write_all(&[0u8; TOKEN_LEN]);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        })
    };
    let start = Instant::now();
    let result = accept_remote(&listener, Duration::from_millis(300), &[7; TOKEN_LEN]);
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    intruder.join().unwrap();
    assert_eq!(result.err().map(|e| e.kind()), Some(io::ErrorKind::TimedOut));
    assert!(start.elapsed() < Duration::from_secs(2), "took {:?}", start.elapsed());
}

/// A peer that connects and says nothing must not cost a full 1 s read
/// timeout past the deadline, or a queue of them would keep `accept_remote`
/// busy for as many seconds as there were peers.
#[test]
fn accept_remote_times_out_under_silent_peers() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    // Held open (not dropped) so each read waits out its timeout, not EOF.
    let silent: Vec<TcpStream> = (0..5).map(|_| TcpStream::connect(("127.0.0.1", port)).unwrap()).collect();
    let start = Instant::now();
    let result = accept_remote(&listener, Duration::from_millis(300), &[7; TOKEN_LEN]);
    drop(silent);
    assert_eq!(result.err().map(|e| e.kind()), Some(io::ErrorKind::TimedOut));
    assert!(start.elapsed() < Duration::from_secs(2), "took {:?}", start.elapsed());
}

/// The engine's inbox is bounded (wave 3b): an engine whose main loop takes nothing stops its reader, which backs
/// the app up on the pipe, and no message is lost.
#[test]
fn the_view_inbox_is_bounded_and_loses_nothing() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    const N: usize = 1000;
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let host = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (view_kind, _) = listener.accept().unwrap();
    let written = Arc::new(AtomicUsize::new(0));
    let progress = Arc::clone(&written);
    std::thread::spawn(move || {
        let mut out = MessageChannel::new(host);
        out.send_version().unwrap();
        for i in 0..N {
            if out.send(&HostToView::SetCookies(format!("{}{i}", "0".repeat(65_530)))).is_err() {
                return;
            }
            progress.store(i + 1, Ordering::SeqCst);
        }
    });
    let rx = spawn_host_reader(MessageChannel::new(view_kind), || {});
    // Proves an absence: the count stops moving once the inbox and the socket buffers are full.
    let mut last = usize::MAX;
    let end = Instant::now() + Duration::from_secs(20);
    while Instant::now() < end {
        std::thread::sleep(Duration::from_millis(300));
        let now = written.load(Ordering::SeqCst);
        if now == last {
            break;
        }
        last = now;
    }
    assert!(last < N, "the app wrote all {N} while the engine took none: the inbox is unbounded");
    for i in 0..N {
        match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
            HostToView::SetCookies(s) => assert_eq!(s.trim_start_matches('0').parse::<usize>().unwrap_or(0), i),
            _ => panic!("message {i} is not SetCookies"),
        }
    }
}

#[test]
fn the_token_round_trips_through_hex() {
    let token: Token = std::array::from_fn(|i| (i * 17) as u8);
    assert_eq!(token_from_hex(&to_hex(&token)), Some(token));
    assert_eq!(token_from_hex("zz"), None);
    assert_eq!(token_from_hex(&"é".repeat(16)), None);
}

#[test]
fn hex_round_trips_any_bytes_and_refuses_anything_else() {
    let text = "https://x/'$(touch${IFS}/tmp/pwn)'\";`id`\n\u{e9}";
    assert_eq!(from_hex(&to_hex(text.as_bytes())).unwrap(), text.as_bytes());
    assert!(to_hex(text.as_bytes()).bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    assert_eq!(from_hex("0A").unwrap(), [10]);
    for bad in ["0", "zz", "+f", "0x", "é0"] {
        assert_eq!(from_hex(bad), None, "{bad}");
    }
    assert_eq!(token_from_hex(&"+f".repeat(16)), None, "a sign is not a hex digit");
}

#[test]
fn only_a_peer_with_the_token_is_accepted() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let token: Token = [9; TOKEN_LEN];
    let intruder = std::thread::spawn(move || {
        let _ = connect_remote("127.0.0.1", port, &[1; TOKEN_LEN]);
    });
    let view_kind = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        connect_remote("127.0.0.1", port, &[9; TOKEN_LEN]).unwrap()
    });
    let mut accepted = accept_remote(&listener, Duration::from_secs(5), &token).expect("the real engine");
    let mut view_kind = view_kind.join().unwrap();
    intruder.join().unwrap();
    // Each side wrote its version as soon as the token was through.
    assert_eq!(accepted.recv_version().unwrap(), crate::PROTOCOL_VERSION);
    assert_eq!(view_kind.recv_version().unwrap(), crate::PROTOCOL_VERSION);
    view_kind.send(&1u8).unwrap();
    assert_eq!(accepted.recv::<u8>().unwrap(), 1);
}
