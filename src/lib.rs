use core::time;
use daemonize::Daemonize;
use dirs::config_dir;
use nix::libc;
use nix::libc::{c_ushort, TIOCSWINSZ};
use nix::sys::wait::waitpid;
use nix::unistd::ForkResult;
use pty::fork::*;
use serde::{Deserialize, Serialize};
use signal_hook::{
    consts::{SIGINT, SIGWINCH},
    iterator::Signals,
};
use std::collections::HashMap;
use std::fs::{self, remove_file};
use std::io::{self, stdout, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::prelude::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::{fmt, thread};
use termion::raw::IntoRawMode;

use anyhow::Context;

fn server(
    socket_path: String,
    command: String,
    args: Vec<String>,
    env: HashMap<String, String>,
) -> anyhow::Result<()> {
    for (k, v) in env {
        std::env::set_var(k, v);
    }
    if std::fs::metadata(&socket_path).is_ok() {
        println!("A socket is already present. Deleting...");
        std::fs::remove_file(&socket_path)
            .with_context(|| format!("could not delete previous socket at {:?}", socket_path))?;
    }

    let unix_listener =
        UnixListener::bind(&socket_path).context("Could not create the unix socket")?;

    let socket_path2 = socket_path.clone();

    let mut signals = Signals::new(&[SIGINT])?;

    thread::spawn(move || {
        for _ in signals.forever() {
            println!("unlink2 {}", &socket_path2);
            remove_file(&socket_path2).unwrap();
        }
    });

    let daemonize = Daemonize::new(); // Redirect stderr to `/tmp/daemon.err`.

    daemonize.start()?;

    let fork = Fork::from_ptmx().unwrap();
    if let Some(master) = fork.is_parent().ok() {
        thread::spawn(move || loop {
            waitpid(None, None).unwrap();
            println!("unlink {}", &socket_path);
            remove_file(&socket_path).unwrap();
            std::process::exit(0);
        });
        // put the server logic in a loop to accept several connections
        loop {
            let (unix_stream, _socket_address) = unix_listener
                .accept()
                .context("Failed at accepting a connection on the unix listener")?;
            handle_stream(unix_stream, master)?;
        }
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

    let mut signals = Signals::new(&[SIGWINCH])?;
    let mut unix_stream_resize = unix_stream.try_clone()?;

    thread::spawn(move || {
        for sig in signals.forever() {
            if sig == SIGWINCH {
                let term_size = Message {
                    mode: 0,
                    size: termion::terminal_size().unwrap(),
                    byte: 0,
                };
                let encoded: Vec<u8> = bincode::serialize(&term_size).unwrap();
                if unix_stream_resize.write_all(&encoded[..]).is_err() {
                    break;
                }
            }
        }
    });

    // send terminal size
    let term_size = Message {
        mode: 0,
        size: termion::terminal_size()?,
        byte: 0,
    };
    let encoded: Vec<u8> = bincode::serialize(&term_size).unwrap();
    unix_stream
        .write_all(&encoded[..])
        .context("Failed at writing the unix stream")?;

    unix_stream.flush()?;
    // send CTRL+L to force redraw
    let term_size = Message {
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

    t2.join().unwrap();

    unix_stream
        .shutdown(std::net::Shutdown::Write)
        .context("Could not shutdown writing on the stream")
}

struct NoConfigDir;

impl fmt::Display for NoConfigDir {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "no config dir")
    }
}

fn conf_dir() -> anyhow::Result<String> {
    let dir = config_dir().ok_or(anyhow::anyhow!("config dir not found"))?;
    let crate_name = option_env!("CARGO_PKG_NAME").unwrap_or("rda");
    let dir_str = dir.as_path().display();
    let confdir = format!("{}/{}", dir_str, crate_name);
    if !Path::new(&confdir).exists() {
        fs::create_dir(&confdir)?;
    }
    Ok(confdir)
}

fn session_name_to_socket_path(session_name: String) -> anyhow::Result<String> {
    let confdir = conf_dir()?;
    Ok(format!("{}/{}", confdir, session_name))
}

pub fn list_sessions() -> anyhow::Result<Vec<String>> {
    let paths = fs::read_dir(conf_dir()?)?;
    let res = paths
        .map(|path| {
            path.unwrap()
                .file_name()
                .to_str()
                .unwrap_or("failed to unwrap file name")
                .to_string()
        })
        .collect();
    Ok(res)
}

fn session_running(session_name: String) -> anyhow::Result<bool> {
    Ok(Path::new(&session_name_to_socket_path(session_name)?).exists())
}

pub fn server_client(
    session_name: &String,
    command: &Vec<String>,
    env: HashMap<String, String>,
) -> anyhow::Result<()> {
    let socket_path = session_name_to_socket_path(session_name.clone())?;
    if session_running(session_name.clone())? {
        client(socket_path)?;
    } else {
        println!("fork");
        let pid = unsafe { nix::unistd::fork() };
        println!("pid: {:?}", pid);
        match pid.expect("Fork Failed: Unable to create child process!") {
            ForkResult::Child => {
                let command_name = command.get(0).unwrap();
                let remaining_args = &command[1..command.len()];
                server(
                    socket_path,
                    command_name.to_string(),
                    remaining_args.to_vec(),
                    env,
                )?;
            }
            ForkResult::Parent { .. } => {
                println!("parent");
                thread::sleep(time::Duration::from_millis(100));
                client(socket_path)?;
            }
        }
    }
    Ok(())
}
