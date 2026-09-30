use std::io::Cursor;

use moon_core::station_api::Answer;

use super::*;

/// A connection in memory: what the client sent, and what the station wrote back.
struct Duplex {
    input: Cursor<Vec<u8>>,
    output: Vec<u8>,
}

impl Read for Duplex {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.input.read(buf)
    }
}

impl Write for Duplex {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.output.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn client_sends(request: &Request) -> Duplex {
    let mut input = Vec::new();
    write_frame(&mut input, request).unwrap();
    Duplex {
        input: Cursor::new(input),
        output: Vec::new(),
    }
}

#[test]
fn a_frame_reads_back_as_written() {
    let mut buf = Vec::new();
    write_frame(&mut buf, &Request::AccessGet).unwrap();
    assert_eq!(&buf[..4], &(buf.len() as u32 - 4).to_be_bytes());
    let back: Request = read_frame(&mut Cursor::new(buf)).unwrap();
    assert_eq!(back, Request::AccessGet);
}

/// A length past the cap is refused before anything is allocated for it.
#[test]
fn an_oversized_frame_is_refused_unread() {
    let mut buf = ((MAX_FRAME + 1) as u32).to_be_bytes().to_vec();
    buf.extend_from_slice(b"{}");
    assert!(read_frame::<Request>(&mut Cursor::new(buf)).is_err());
}

/// Hello first, then the main loop's own answer to the request.
#[test]
fn an_exchange_says_hello_and_relays_the_loops_answer() {
    let (tx, rx) = mpsc::channel::<Call>();
    let answering = std::thread::spawn(move || {
        let call = rx.recv().unwrap();
        assert_eq!(call.request, Request::PairIssue);
        call.reply.send(Reply::Err("no bot".into())).unwrap();
    });
    let mut conn = client_sends(&Request::PairIssue);
    exchange(&mut conn, &tx, Duration::from_secs(5)).unwrap();
    answering.join().unwrap();
    let mut out = Cursor::new(conn.output);
    let hello: Hello = read_frame(&mut out).unwrap();
    assert_eq!(hello.proto_version, PROTO_VERSION);
    let reply: Reply = read_frame(&mut out).unwrap();
    assert_eq!(reply, Reply::Err("no bot".into()));
}

/// A loop that does not answer in time still gets the client a reply — never a hang.
#[test]
fn a_loop_that_does_not_answer_times_out_with_a_reason() {
    let (tx, _rx) = mpsc::channel::<Call>();
    let mut conn = client_sends(&Request::Status);
    exchange(&mut conn, &tx, Duration::from_millis(20)).unwrap();
    let mut out = Cursor::new(conn.output);
    let _: Hello = read_frame(&mut out).unwrap();
    let reply: Reply = read_frame(&mut out).unwrap();
    assert!(matches!(reply, Reply::Err(reason) if reason.contains("nothing was changed")));
}

/// A request whose client was told "nothing was changed" is never answered — so never applied.
#[test]
fn drain_skips_a_request_past_its_deadline() {
    let (tx, calls) = mpsc::channel();
    let api = Api {
        calls,
        path: std::env::temp_dir().join(format!("moon-station-api-late-{}", std::process::id())),
    };
    let (reply_tx, _reply_rx) = mpsc::sync_channel(1);
    tx.send(Call {
        request: Request::AccessGet,
        reply: reply_tx,
        deadline: Instant::now(),
    })
    .unwrap();
    api.drain(|_| panic!("a late request must not be answered"));
}

/// Garbage from the client still gets a reply saying so.
#[test]
fn an_unreadable_request_is_answered_with_a_reason() {
    let (tx, _rx) = mpsc::channel::<Call>();
    let mut input = Vec::new();
    write_frame(&mut input, &serde_json::json!({"cmd": "tape.fetch"})).unwrap();
    let mut conn = Duplex {
        input: Cursor::new(input),
        output: Vec::new(),
    };
    assert!(exchange(&mut conn, &tx, Duration::from_secs(1)).is_err());
    let mut out = Cursor::new(conn.output);
    let _: Hello = read_frame(&mut out).unwrap();
    let reply: Reply = read_frame(&mut out).unwrap();
    assert!(matches!(reply, Reply::Err(reason) if reason.contains("unreadable")));
}

/// Requests wait until the main loop drains them, and each gets its own answer.
#[test]
fn drain_answers_every_waiting_request() {
    let (tx, calls) = mpsc::channel();
    let api = Api {
        calls,
        path: std::env::temp_dir().join(format!("moon-station-api-{}", std::process::id())),
    };
    let (reply_tx, reply_rx) = mpsc::sync_channel(1);
    tx.send(Call {
        request: Request::AccessGet,
        reply: reply_tx,
        deadline: Instant::now() + Duration::from_secs(60),
    })
    .unwrap();
    api.drain(|request| {
        assert_eq!(request, Request::AccessGet);
        Reply::Ok(Answer::Access(Default::default()))
    });
    assert_eq!(
        reply_rx.try_recv().unwrap(),
        Reply::Ok(Answer::Access(Default::default()))
    );
}
