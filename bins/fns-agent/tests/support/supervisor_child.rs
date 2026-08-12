use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

fn main() {
    let mode = std::env::args().nth(1).expect("mode");
    let marker = std::env::args().nth(2).map(PathBuf::from);
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    let _bootstrap = read_frame(&mut input).expect("bootstrap");

    match mode.as_str() {
        "fatal-before-ready" => {
            write_frame(&mut output, br#"{"type":"fatal","code":"core"}"#);
        }
        "never-ready" => loop {
            std::thread::sleep(Duration::from_secs(1));
        },
        "abnormal-after-ready" => {
            write_frame(&mut output, br#"{"type":"ready"}"#);
            std::process::exit(23);
        }
        "record-process-data" => {
            const SENTINEL: &str = "FNS_SENTINEL_TOKEN_6c26289d";
            let argv_contains_token = std::env::args().any(|value| value.contains(SENTINEL));
            let environment_contains_token =
                std::env::vars_os().any(|(_, value)| value.to_string_lossy().contains(SENTINEL));
            let observed = format!(
                "argv_contains_token={argv_contains_token}\nenvironment_contains_token={environment_contains_token}\n"
            );
            fs::write(marker.expect("marker"), observed).expect("write marker");
            write_frame(&mut output, br#"{"type":"ready"}"#);
            let _ = read_frame(&mut input);
            write_frame(&mut output, br#"{"type":"stopped"}"#);
        }
        "ignore-shutdown" => {
            write_frame(&mut output, br#"{"type":"ready"}"#);
            let _ = read_frame(&mut input);
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        "exit-on-eof" => {
            write_frame(&mut output, br#"{"type":"ready"}"#);
            while read_frame(&mut input).is_some() {}
        }
        "quiesce-then-stop" => {
            write_frame(&mut output, br#"{"type":"ready"}"#);
            let _ = read_frame(&mut input).expect("shutdown");
            fs::write(marker.expect("marker"), b"quiesced").expect("write marker");
            write_frame(&mut output, br#"{"type":"stopped"}"#);
        }
        "malformed-after-ready" => {
            write_frame(&mut output, br#"{"type":"ready"}"#);
            let _ = read_frame(&mut input).expect("shutdown");
            write_frame(&mut output, b"nope");
        }
        "list-fail-list" => {
            write_frame(&mut output, br#"{"type":"ready"}"#);
            let first = frame_request_id(&read_frame(&mut input).expect("first list"));
            write_frame(
                &mut output,
                format!(r#"{{"type":"conflicts_listed","requestId":"{first}","conflicts":[]}}"#)
                    .as_bytes(),
            );
            let second = frame_request_id(&read_frame(&mut input).expect("resolve"));
            write_frame(
                &mut output,
                format!(r#"{{"type":"request_failed","requestId":"{second}","code":"core"}}"#)
                    .as_bytes(),
            );
            let third = frame_request_id(&read_frame(&mut input).expect("second list"));
            write_frame(
                &mut output,
                format!(r#"{{"type":"conflicts_listed","requestId":"{third}","conflicts":[]}}"#)
                    .as_bytes(),
            );
            let _ = read_frame(&mut input).expect("shutdown");
            write_frame(&mut output, br#"{"type":"stopped"}"#);
        }
        "wrong-request-id" => {
            write_frame(&mut output, br#"{"type":"ready"}"#);
            let _ = read_frame(&mut input).expect("list");
            write_frame(
                &mut output,
                br#"{"type":"conflicts_listed","requestId":"90000000-0000-4000-8000-000000000099","conflicts":[]}"#,
            );
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        "duplicate-response" => {
            write_frame(&mut output, br#"{"type":"ready"}"#);
            let request_id = frame_request_id(&read_frame(&mut input).expect("list"));
            let response = format!(
                r#"{{"type":"conflicts_listed","requestId":"{request_id}","conflicts":[]}}"#
            );
            write_frame(&mut output, response.as_bytes());
            write_frame(&mut output, response.as_bytes());
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        "rpc-timeout" => {
            write_frame(&mut output, br#"{"type":"ready"}"#);
            let _ = read_frame(&mut input).expect("list");
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        "fatal-during-rpc" => {
            write_frame(&mut output, br#"{"type":"ready"}"#);
            let _ = read_frame(&mut input).expect("list");
            write_frame(&mut output, br#"{"type":"fatal","code":"core"}"#);
        }
        "stopped-during-rpc" => {
            write_frame(&mut output, br#"{"type":"ready"}"#);
            let _ = read_frame(&mut input).expect("list");
            write_frame(&mut output, br#"{"type":"stopped"}"#);
        }
        "eof-during-rpc" => {
            write_frame(&mut output, br#"{"type":"ready"}"#);
            let _ = read_frame(&mut input).expect("list");
        }
        "truncated-during-rpc" => {
            write_frame(&mut output, br#"{"type":"ready"}"#);
            let _ = read_frame(&mut input).expect("list");
            output.write_all(&10_u32.to_be_bytes()).expect("length");
            output.write_all(b"{}").expect("partial payload");
            output.flush().expect("flush partial payload");
        }
        "oversized-during-rpc" => {
            write_frame(&mut output, br#"{"type":"ready"}"#);
            let _ = read_frame(&mut input).expect("list");
            output
                .write_all(&1_048_577_u32.to_be_bytes())
                .expect("oversized length");
            output.flush().expect("flush oversized length");
        }
        other => panic!("unknown mode {other}"),
    }
}

fn frame_request_id(frame: &[u8]) -> String {
    let text = std::str::from_utf8(frame).expect("utf8 request");
    let key = "\"requestId\":\"";
    let start = text.find(key).expect("requestId") + key.len();
    let end = text[start..].find('"').expect("requestId end") + start;
    text[start..end].to_owned()
}

fn read_frame(input: &mut impl Read) -> Option<Vec<u8>> {
    let mut length = [0_u8; 4];
    input.read_exact(&mut length).ok()?;
    let mut payload = vec![0_u8; u32::from_be_bytes(length) as usize];
    input.read_exact(&mut payload).ok()?;
    Some(payload)
}

fn write_frame(output: &mut impl Write, payload: &[u8]) {
    output
        .write_all(&(payload.len() as u32).to_be_bytes())
        .expect("write frame length");
    output.write_all(payload).expect("write frame payload");
    output.flush().expect("flush frame");
}
