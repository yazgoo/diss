use daemonize::Daemonize;
use nix::libc::{c_ushort, TIOCGWINSZ, TIOCSWINSZ};
use nix::sys::ioctl;
use nix::{ioctl_write_ptr, libc};
use pty::fork::*;
use serde::{Deserialize, Serialize};
use signal_hook::{consts::SIGWINCH, iterator::Signals};
use std::fs::File;
use std::io::{self, stdout, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::prelude::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::{mem, thread};
use termion::raw::IntoRawMode;

use anyhow::Context;

fn server(socket_path: String, command: String, args: Vec<String>) -> anyhow::Result<()> {
    let stdout = File::create("/tmp/daemon.out").unwrap();
    let stderr = File::create("/tmp/daemon.err").unwrap();

    if std::fs::metadata(&socket_path).is_ok() {
        println!("A socket is already present. Deleting...");
        std::fs::remove_file(&socket_path)
            .with_context(|| format!("could not delete previous socket at {:?}", socket_path))?;
    }

    let unix_listener =
        UnixListener::bind(socket_path).context("Could not create the unix socket")?;

    let daemonize = Daemonize::new()
        .stdout(stdout) // Redirect stdout to `/tmp/daemon.out`.
        .stderr(stderr); // Redirect stderr to `/tmp/daemon.err`.

    daemonize.start()?;

    let fork = Fork::from_ptmx().unwrap();
    if let Some(mut master) = fork.is_parent().ok() {
        // put the server logic in a loop to accept several connections
        loop {
            println!("listening for new clients");
            let (unix_stream, _socket_address) = unix_listener
                .accept()
                .context("Failed at accepting a connection on the unix listener")?;
            handle_stream(unix_stream, master)?;
        }
        // fork.wait()?;
    } else {
        Command::new(command).args(args).exec();
    }
    Ok(())
}

#[derive(Serialize, Deserialize, Debug)]
struct Message {
    mode: u8,
    size: (u16, u16),
    byte: u8,
}

#[derive(Debug)]
#[repr(C)]
struct UnixSize {
    ws_row: c_ushort,
    ws_col: c_ushort,
    ws_xpixel: c_ushort,
    ws_ypixel: c_ushort,
}

fn handle_stream(mut unix_stream: UnixStream, mut master: Master) -> anyhow::Result<()> {
    let mut bytesr = [0; 1];

    let m: Message = Message {
        mode: 0,
        size: (0, 0),
        byte: 0,
    };
    let len = bincode::serialized_size(&m).unwrap() as usize;

    let mut master_reader = master.clone();
    let mut unix_stream_reader = unix_stream.try_clone()?;
    let fd = master.as_raw_fd();
    thread::spawn(move || {
        let mut bytes = vec![0; len];
        loop {
            let res = unix_stream_reader.read_exact(&mut bytes);
            if res.is_err() {
                break;
            }
            res.context("Failed at reading the unix stream").unwrap();
            let message: Message = bincode::deserialize_from(&bytes[..])
                .context("failed at deseriazing bytes")
                .unwrap();
            if message.mode == 0 {
                let us = UnixSize {
                    ws_row: message.size.1,
                    ws_col: message.size.0,
                    ws_xpixel: 0,
                    ws_ypixel: 0,
                };
                unsafe {
                    libc::ioctl(fd, TIOCSWINSZ, &us);
                };
            } else if message.mode == 1 {
                if master.write(&[message.byte]).is_err() {
                    break;
                }
            } else if message.mode == 2 {
                // detach
            }
        }
    });
    thread::spawn(move || loop {
        let _size = master_reader
            .read(&mut bytesr)
            .context("failed at reading stdout")
            .unwrap();
        if _size > 0 {
            let res = unix_stream.write(&bytesr);
            if res.is_err() {
                break;
            }
            res.context("Failed at writing the unix stream").unwrap();
        } else {
            break;
        }
    });

    Ok(())
}

fn client(socket_path: String) -> anyhow::Result<()> {
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
            match unix_stream_reader.read(&mut bytes) {
                Ok(_size) => {
                    if _size > 0 {
                        _stdout
                            .write(&bytes)
                            .context("failed at writing stdin")
                            .unwrap();
                        _stdout.flush().unwrap();
                    }
                }
                Err(_) => break,
            }
        }
    });

    let term = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGWINCH, Arc::clone(&term))?;
    let mut unix_stream_resize = unix_stream.try_clone()?;

    let t1 = thread::spawn(move || {
        while !term.load(std::sync::atomic::Ordering::Relaxed) {
            let mut term_size = Message {
                mode: 0,
                size: termion::terminal_size().unwrap(),
                byte: 0,
            };
            let encoded: Vec<u8> = bincode::serialize(&term_size).unwrap();
            if unix_stream_resize.write_all(&encoded[..]).is_err() {
                break;
            }
        }
    });

    // send terminal size
    let mut term_size = Message {
        mode: 0,
        size: termion::terminal_size()?,
        byte: 0,
    };
    let encoded: Vec<u8> = bincode::serialize(&term_size).unwrap();
    unix_stream
        .write_all(&encoded[..])
        .context("Failed at writing the unix stream")?;
    let mut unix_stream_stdin = unix_stream.try_clone()?;

    unix_stream.flush()?;
    // send CTRL+L to force redraw
    let mut term_size = Message {
        mode: 1,
        size: (0, 0),
        byte: 12,
    };
    let encoded: Vec<u8> = bincode::serialize(&term_size).unwrap();
    unix_stream
        .write_all(&encoded[..])
        .context("Failed at writing the unix stream")?;
    let mut unix_stream_stdin = unix_stream.try_clone()?;

    let t2 = thread::spawn(move || loop {
        let _size = stdin
            .read(&mut bytesr)
            .context("failed at reading stdout")
            .unwrap();
        if _size > 0 {
            if bytesr[0] == 4 {
                // detach
                let message = Message {
                    mode: 2,
                    size: (0, 0),
                    byte: bytesr[0],
                };
                let encoded: Vec<u8> = bincode::serialize(&message).unwrap();
                unix_stream_stdin
                    .write_all(&encoded[..])
                    .context("Failed at writing the unix stream")
                    .unwrap();
                unix_stream_stdin
                    .shutdown(std::net::Shutdown::Write)
                    .context("Could not shutdown writing on the stream")
                    .unwrap();
            }
            let message = Message {
                mode: 1,
                size: (0, 0),
                byte: bytesr[0],
            };
            let encoded: Vec<u8> = bincode::serialize(&message).unwrap();
            let res = unix_stream_stdin.write_all(&encoded[..]);
            if res.is_err() {
                break;
            }
            res.context("Failed at writing the unix stream").unwrap();
        }
    });

    t1.join();

    unix_stream
        .shutdown(std::net::Shutdown::Write)
        .context("Could not shutdown writing on the stream")?;

    std::process::exit(0);
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let arg1 = std::env::args().nth(1);
    let socket_path = std::env::args().nth(2).unwrap();
    match arg1 {
        Some(action) if action == "server" => {
            let command = std::env::args().nth(3).unwrap();
            let all_args: Vec<String> = std::env::args().collect();
            let remaining_args = &all_args[4..all_args.len()];
            server(socket_path, command, remaining_args.to_vec())
        }
        Some(action) if action == "client" => client(socket_path),
        _ => Ok(()),
    }
}
