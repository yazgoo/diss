use bincode::{DefaultOptions, Options};
use core::time;
use daemonize::Daemonize;
use nix::libc;
use nix::libc::{c_ushort, TIOCSWINSZ};
use nix::sys::signal::kill;
use nix::sys::wait::waitpid;
use nix::unistd::{ForkResult, Pid};
use pty::fork::*;
use serde::{Deserialize, Serialize};
use signal_hook::{
    consts::{SIGINT, SIGTERM, SIGWINCH},
    iterator::Signals,
};
use std::collections::HashMap;
use std::fs::{self, remove_file};
use std::io::{self, stdout, Read};
use std::io::{ErrorKind, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::prelude::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use std::{env, fmt, thread};
use termion::raw::IntoRawMode;
use timeout_readwrite::TimeoutReader;

use anyhow::Context;
use log::{debug, info};
use log_derive::{logfn, logfn_inputs};

#[logfn(Debug)]
#[logfn_inputs(Debug)]
fn server(
    socket_path: String,
    command: String,
    args: Vec<String>,
    env: HashMap<String, String>,
) -> anyhow::Result<()> {
    env.get("PWD")
        .map(|d| {
            debug!("current dir: {}", d);
            d
        })
        .map(env::set_current_dir);
    for (k, v) in env {
        debug!("env var: {} = {}", k, v);
        env::set_var(k, v);
    }
    if std::fs::metadata(&socket_path).is_ok() {
        info!("A socket is already present. Deleting...");
        std::fs::remove_file(&socket_path)
            .with_context(|| format!("could not delete previous socket at {:?}", socket_path))?;
    }

    debug!("binding to {:?}", socket_path);
    let unix_listener =
        UnixListener::bind(&socket_path).context("Could not create the unix socket")?;

    let socket_path2 = socket_path.clone();

    start_thread_to_cleanup_unix_socket_on_shutdown(socket_path2)?;

    let dir = env::current_dir()?;
    Daemonize::new().working_directory(dir).start()?;

    match Fork::from_ptmx()? {
        Fork::Parent(pid, master) => {
            run_server_parent_process(pid, master, socket_path, unix_listener)?;
        }
        Fork::Child(_) => {
            Command::new(command).args(args).exec();
        }
    }
    Ok(())
}

#[logfn_inputs(Debug)]
fn run_server_parent_process(
    pid: i32,
    master: Master,
    socket_path: String,
    unix_listener: UnixListener,
) -> anyhow::Result<()> {
    start_thread_to_cleanup_unix_socket_on_process_status_change(socket_path);

    let master_readers: Arc<RwLock<Vec<UnixStream>>> = Arc::new(RwLock::new(vec![]));
    let master_readers_2 = master_readers.clone();

    start_thread_to_send_data_from_forked_process_to_clients(master, master_readers);

    // put the server logic in a loop to accept several connections
    loop {
        let (unix_stream, _socket_address) = unix_listener
            .accept()
            .context("Failed at accepting a connection on the unix listener")?;

        /* add client to the list of clients */
        let mut r = master_readers_2.write().unwrap();
        r.push(unix_stream.try_clone().unwrap());

        start_thread_to_handle_clients_messages(
            master,
            pid,
            TimeoutReader::new(unix_stream.try_clone()?, Duration::from_millis(10)),
        );
    }
}

#[logfn(Debug)]
#[logfn_inputs(Debug)]
fn start_thread_to_send_data_from_forked_process_to_clients(
    master: Master,
    master_readers: Arc<RwLock<Vec<UnixStream>>>,
) {
    // forked-process > unix stream
    let mut master_reader = master;
    let mut bytesr = [0; 1024];
    thread::spawn(move || loop {
        let mut should_sleep = false;
        {
            let mut to_remove = None;
            match master_reader.read(&mut bytesr) {
                // start
                Ok(size) => match master_readers.try_write() {
                    Err(_) => {
                        should_sleep = true;
                    }
                    Ok(mut for_read) => {
                        if for_read.len() == 0 {
                            should_sleep = true;
                        } else if size > 0 {
                            for (i, unix_stream) in for_read.iter_mut().enumerate() {
                                let res = unix_stream.write(&bytesr[0..size]);
                                if res.is_err() {
                                    to_remove = Some(i);
                                }
                            }
                        }
                        to_remove.map(|i| for_read.remove(i));
                    }
                },
                Err(_) => {
                    break;
                }
            }
        }
        if should_sleep {
            thread::sleep(time::Duration::from_millis(10));
        }
    });
}

#[logfn(Debug)]
fn start_thread_to_handle_clients_messages(
    mut master2: Master,
    pid: i32,
    mut reader: TimeoutReader<UnixStream>,
) {
    thread::spawn(move || {
        'outer: loop {
            match receive_message(&mut reader) {
                Ok(message) => {
                    // start
                    if message.mode == 0 {
                        let us = UnixSize {
                            ws_row: message.size.1,
                            ws_col: message.size.0,
                            ws_xpixel: 0,
                            ws_ypixel: 0,
                        };
                        unsafe {
                            libc::ioctl(master2.as_raw_fd(), TIOCSWINSZ, &us);
                        };
                    } else if message.mode == 1 {
                        if master2.write(&message.bytes).is_err() {
                            break;
                        }
                    } else if message.mode == 2 {
                        // detach
                    } else if message.mode == 3 {
                        // redraw
                        let r = kill(Pid::from_raw(pid), nix::sys::signal::SIGWINCH);
                        debug!("redraw result: {:?}", r);
                    }
                    // end
                }
                Err(ref e) if underlying_io_error_kind(e) == Some(ErrorKind::TimedOut) => {
                    // should_sleep = true
                }
                Err(_) => {
                    break 'outer;
                }
            }
        }
    });
}
#[derive(Debug)]
#[repr(C)]
struct UnixSize {
    ws_row: c_ushort,
    ws_col: c_ushort,
    ws_xpixel: c_ushort,
    ws_ypixel: c_ushort,
}

#[logfn(Debug)]
#[logfn_inputs(Debug)]
fn start_thread_to_cleanup_unix_socket_on_process_status_change(socket_path: String) {
    thread::spawn(move || loop {
        let res = waitpid(None, None).unwrap();
        debug!(
            "start_thread_to_cleanup_unix_socket_on_process_status_change: unlink {} {:?}",
            &socket_path, res
        );
        remove_file(&socket_path).unwrap();
        std::process::exit(0);
    });
}

#[logfn(Debug)]
#[logfn_inputs(Debug)]
fn start_thread_to_cleanup_unix_socket_on_shutdown(socket_path: String) -> anyhow::Result<()> {
    let mut signals = Signals::new([SIGINT, SIGTERM])?;
    thread::spawn(move || {
        for signal in signals.forever() {
            debug!(
                "start_thread_to_cleanup_unix_socket_on_shutdown: unlink {}, signal: {}",
                &socket_path, signal
            );
            println!("unlink2 {}", &socket_path);
            remove_file(&socket_path).unwrap();
        }
    });
    Ok(())
}

#[derive(Serialize, Deserialize, Debug)]
struct Message {
    mode: u8,
    size: (u16, u16),
    bytes: Vec<u8>,
}

#[logfn(Debug)]
#[logfn_inputs(Debug)]
fn send_message(unix_stream: &mut UnixStream, message: &Message) -> anyhow::Result<()> {
    let encoded = DefaultOptions::new()
        .with_varint_encoding()
        .serialize(&message)?;
    unix_stream.write_all(&[encoded.len() as u8])?;
    unix_stream.write_all(&encoded[..]).map_err(|x| x.into())
}

fn receive_message(unix_stream_reader: &mut TimeoutReader<UnixStream>) -> anyhow::Result<Message> {
    let mut len_array = vec![0; 1];
    unix_stream_reader.read_exact(&mut len_array)?;
    let mut bytes = vec![0; len_array[0].into()];
    unix_stream_reader.read_exact(&mut bytes)?;
    DefaultOptions::new()
        .with_varint_encoding()
        .deserialize_from(&bytes[..])
        .map_err(|x| x.into())
}

pub fn underlying_io_error_kind(error: &anyhow::Error) -> Option<io::ErrorKind> {
    for cause in error.chain() {
        if let Some(io_error) = cause.downcast_ref::<io::Error>() {
            return Some(io_error.kind());
        }
    }
    None
}

#[logfn(Debug)]
#[logfn_inputs(Debug)]
fn escape_key_to_byte(escape_key: Option<String>) -> u8 {
    let allowed_keys = vec![
        "a".to_string(),
        "b".to_string(),
        "c".to_string(),
        "d".to_string(),
        "e".to_string(),
        "f".to_string(),
        "g".to_string(),
    ];
    escape_key
        .map(|x| {
            allowed_keys
                .iter()
                .position(|y| y == &x)
                .map(|i| i as u8 + 1)
                .unwrap_or(1)
        })
        .unwrap_or(7) /* default escape key == g */
}

#[logfn(Debug)]
#[logfn_inputs(Debug)]
fn client(socket_path: String, escape_key: Option<String>) -> anyhow::Result<()> {
    let mut unix_stream = UnixStream::connect(socket_path).context("Could not create stream")?;

    write_request_and_shutdown(&mut unix_stream, escape_key_to_byte(escape_key))?;
    // read_from_stream(&mut unix_stream)?;
    Ok(())
}

#[logfn(Debug)]
#[logfn_inputs(Debug)]
fn write_request_and_shutdown(unix_stream: &mut UnixStream, escape_code: u8) -> anyhow::Result<()> {
    let mut _stdout = stdout().into_raw_mode()?;
    let mut stdin = TimeoutReader::new(io::stdin(), Duration::from_millis(50));

    let mut unix_stream_reader =
        TimeoutReader::new(unix_stream.try_clone()?, Duration::from_millis(50));

    print!("{}[2J", 27 as char);
    let mut _stdout2 = stdout();
    let done = Arc::new(AtomicBool::new(false));

    let done_in = done.clone();
    thread::spawn(move || {
        let mut bytes = [0; 255];
        loop {
            match unix_stream_reader.read(&mut bytes) {
                Ok(_size) => {
                    if _size > 0 {
                        _stdout2
                            .write(&bytes[0.._size])
                            .context("failed at writing stdin")
                            .unwrap();
                        _stdout2.flush().unwrap();
                    } else {
                        done_in.store(true, Ordering::Relaxed);
                        break;
                    }
                }
                Err(ref e) if e.kind() == ErrorKind::TimedOut => {}
                Err(_) => {
                    done_in.store(true, Ordering::Relaxed);
                    break;
                }
            }
        }
    });

    let mut signals = Signals::new([SIGWINCH])?;
    let mut unix_stream_resize = unix_stream.try_clone()?;

    thread::spawn(move || {
        for sig in signals.forever() {
            if sig == SIGWINCH
                && send_message(
                    &mut unix_stream_resize,
                    &Message {
                        mode: 0,
                        size: termion::terminal_size().unwrap(),
                        bytes: vec![0],
                    },
                )
                .is_err()
            {
                break;
            }
        }
    });

    // send terminal size
    send_message(
        unix_stream,
        &Message {
            mode: 0,
            size: termion::terminal_size()?,
            bytes: vec![0],
        },
    )
    .context("Failed at writing the unix stream")?;

    let ctrl_l = vec![12];
    // send CTRL+L to force redraw
    send_message(
        unix_stream,
        &Message {
            mode: 1,
            size: (0, 0),
            bytes: ctrl_l,
        },
    )
    .context("Failed at writing the unix stream")?;

    let mut unix_stream_stdin = unix_stream.try_clone()?;

    let mut bytesr = [0; 1024];
    let done_stdin = done;
    let t2 = thread::spawn(move || 'outer: loop {
        match stdin.read(&mut bytesr) {
            Ok(_size) => {
                if _size > 0 {
                    if _size == 1 && bytesr[0] == escape_code {
                        // detach
                        send_message(
                            &mut unix_stream_stdin,
                            &Message {
                                mode: 2,
                                size: (0, 0),
                                bytes: vec![bytesr[0]],
                            },
                        )
                        .context("Failed at writing the unix stream")
                        .unwrap();
                        unix_stream_stdin
                            .shutdown(std::net::Shutdown::Write)
                            .context("Could not shutdown writing on the stream")
                            .unwrap();
                    }
                    let res = send_message(
                        &mut unix_stream_stdin,
                        &Message {
                            mode: 1,
                            size: (0, 0),
                            bytes: bytesr[.._size].into(),
                        },
                    );
                    if res.is_err() {
                        break 'outer;
                    }
                }
            }
            Err(ref e) if e.kind() == ErrorKind::TimedOut => {
                if done_stdin.load(Ordering::Relaxed) {
                    break 'outer;
                }
            }
            Err(_) => break,
        }
    });

    t2.join().unwrap();

    let _ = unix_stream.shutdown(std::net::Shutdown::Write);

    let _ = unix_stream.shutdown(std::net::Shutdown::Read);

    Ok(())
}

struct NoConfigDir;

impl fmt::Display for NoConfigDir {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "no config dir")
    }
}

#[logfn(Debug)]
#[logfn_inputs(Debug)]
fn conf_dir() -> anyhow::Result<String> {
    let dir = std::env::temp_dir();
    let crate_name = option_env!("CARGO_PKG_NAME").unwrap_or("rda");
    let dir_str = dir.as_path().display();
    let user = whoami::username();
    let confdir = format!("{}/{}.{}", dir_str, user, crate_name);
    if !Path::new(&confdir).exists() {
        fs::create_dir(&confdir)?;
    }
    Ok(confdir)
}

#[logfn(Debug)]
#[logfn_inputs(Debug)]
fn session_name_to_socket_path(session_name: String) -> anyhow::Result<String> {
    let confdir = conf_dir()?;
    Ok(format!("{}/{}", confdir, session_name))
}

#[logfn(Debug)]
#[logfn_inputs(Debug)]
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

#[logfn(Debug)]
#[logfn_inputs(Debug)]
fn session_running(session_name: String) -> anyhow::Result<bool> {
    Ok(Path::new(&session_name_to_socket_path(session_name)?).exists())
}

#[logfn(Debug)]
#[logfn_inputs(Debug)]
pub fn run(
    session_name: &str,
    command: &[String],
    env: HashMap<String, String>,
    escape_key: Option<String>,
) -> anyhow::Result<()> {
    let socket_path = session_name_to_socket_path(session_name.to_string())?;
    if session_running(session_name.to_string())? {
        client(socket_path, escape_key)?;
    } else {
        let pid = unsafe { nix::unistd::fork() };
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
                thread::sleep(time::Duration::from_millis(10));
                client(socket_path, escape_key)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod test {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn test_list_sessions() {
        let res = list_sessions();
        println!("res : {:?}", res);
        assert!(res.is_ok());
    }

    #[test]
    fn test_new_session_ending_directly() {
        let start = SystemTime::now();
        let session_name = format!(
            "diss-test-ending-{:?}",
            start
                .duration_since(UNIX_EPOCH)
                .expect("should not happen")
                .as_millis()
        );
        let res = run(
            &session_name,
            &["sleep".into(), "0.2".into()],
            HashMap::new(),
            Some("a".into()),
        );
        println!("res : {} {:?}", session_name, res);
        // TODO find a way to test pty even in github actions
        if std::env::var("GITHUB_ACTION").is_err() {
            assert!(res.is_ok());
        }
    }
}
