use pty::fork::*;
use serde::{Deserialize, Serialize};
use std::io::{self, stdout, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::{mem, thread};
use termion::raw::IntoRawMode;

use anyhow::Context;

fn server() -> anyhow::Result<()> {
    let socket_path = "mysocket";

    if std::fs::metadata(socket_path).is_ok() {
        println!("A socket is already present. Deleting...");
        std::fs::remove_file(socket_path)
            .with_context(|| format!("could not delete previous socket at {:?}", socket_path))?;
    }

    let unix_listener =
        UnixListener::bind(socket_path).context("Could not create the unix socket")?;

    // put the server logic in a loop to accept several connections
    loop {
        let (unix_stream, _socket_address) = unix_listener
            .accept()
            .context("Failed at accepting a connection on the unix listener")?;
        handle_stream(unix_stream)?;
    }
    // Ok(())
}

#[derive(Serialize, Deserialize, Debug)]
struct Message {
    mode: u8,
    size: (u16, u16),
    byte: u8,
}

fn handle_stream(mut unix_stream: UnixStream) -> anyhow::Result<()> {
    let mut bytesr = [0; 1];

    let fork = Fork::from_ptmx().unwrap();
    print!("{}[2J", 27 as char);
    stdout().flush()?;

    let m: Message = Message {
        mode: 0,
        size: (0, 0),
        byte: 0,
    };
    let len = bincode::serialized_size(&m).unwrap() as usize;

    if let Some(mut master) = fork.is_parent().ok() {
        let mut master_reader = master.clone();
        let mut unix_stream_reader = unix_stream.try_clone()?;
        thread::spawn(move || {
            let mut bytes = vec![0; len];
            loop {
                unix_stream_reader
                    .read_exact(&mut bytes)
                    .context("Failed at reading the unix stream")
                    .unwrap();
                let message: Message = bincode::deserialize_from(&bytes[..])
                    .context("failed at deseriazing bytes")
                    .unwrap();
                if message.mode == 0 {
                    /* TODO: send resize to slave
                    let resize_cmd =
                        format!("\x1b[8;{};{}t", message.size.0, message.size.1).into_bytes();
                    master
                        .write_all(&resize_cmd[..])
                        .context("failed at writing stdin")
                        .unwrap();
                    */
                } else if message.mode == 1 {
                    master
                        .write(&[message.byte])
                        .context("failed at writing stdin")
                        .unwrap();
                }
            }
        });
        loop {
            let _size = master_reader
                .read(&mut bytesr)
                .context("failed at reading stdout")?;
            if _size > 0 {
                let _size = unix_stream
                    .write(&bytesr)
                    .context("Failed at writing the unix stream")?;
            }
        }
    } else {
        Command::new("/bin/vim")
            .args(vec!["-u", "NONE", "monfichier2"])
            .exec();
    }

    Ok(())
}

fn client() -> anyhow::Result<()> {
    let socket_path = "mysocket";

    let mut unix_stream = UnixStream::connect(socket_path).context("Could not create stream")?;

    write_request_and_shutdown(&mut unix_stream)?;
    // read_from_stream(&mut unix_stream)?;
    Ok(())
}

fn write_request_and_shutdown(unix_stream: &mut UnixStream) -> anyhow::Result<()> {
    let mut _stdout = stdout().into_raw_mode()?;
    let mut bytesr = [0; 1];
    let mut stdin = io::stdin();

    let mut unix_stream_reader = unix_stream.try_clone()?;

    print!("{}[2J", 27 as char);
    thread::spawn(move || {
        let mut bytes = [0; 1];
        loop {
            let _size = unix_stream_reader
                .read(&mut bytes)
                .context("Failed at reading the unix stream")
                .unwrap();
            if _size > 0 {
                _stdout
                    .write(&bytes)
                    .context("failed at writing stdin")
                    .unwrap();
                _stdout.flush().unwrap();
            }
        }
    });
    let term_size = Message {
        mode: 0,
        size: termion::terminal_size()?,
        byte: 0,
    };
    let encoded: Vec<u8> = bincode::serialize(&term_size).unwrap();
    unix_stream
        .write_all(&encoded[..])
        .context("Failed at writing the unix stream")?;
    loop {
        let _size = stdin
            .read(&mut bytesr)
            .context("failed at reading stdout")?;
        if _size > 0 {
            let message = Message {
                mode: 1,
                size: (0, 0),
                byte: bytesr[0],
            };
            let encoded: Vec<u8> = bincode::serialize(&message).unwrap();
            unix_stream
                .write_all(&encoded[..])
                .context("Failed at writing the unix stream")?;
        }
    }

    /*
        unix_stream
            .shutdown(std::net::Shutdown::Write)
            .context("Could not shutdown writing on the stream")?;

    */
}

fn main() -> anyhow::Result<()> {
    let arg1 = std::env::args().nth(1);
    match arg1 {
        Some(action) if action == "server" => server(),
        Some(action) if action == "client" => client(),
        _ => Ok(()),
    }
}
