use pty::fork::*;
use std::io::{self, stdout, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::{Command, Stdio};
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

fn handle_stream(mut unix_stream: UnixStream) -> anyhow::Result<()> {
    let mut bytes = [0; 1];
    let mut bytesr = [0; 1];

    let fork = Fork::from_ptmx().unwrap();

    if let Some(mut master) = fork.is_parent().ok() {
        // Read output via PTY master
        let mut output = String::new();

        match master.read_to_string(&mut output) {
            Ok(_nread) => println!("child tty is: {}", output.trim()),
            Err(e) => panic!("read error: {}", e),
        }
    } else {
        let mut cmd = Command::new("/bin/vim")
            .args(vec!["monfichier"])
            .stdout(Stdio::piped())
            .stdin(Stdio::piped())
            .spawn()?;

        {
            let stdout = cmd.stdout.as_mut().unwrap();
            let stdin = cmd.stdin.as_mut().unwrap();
            loop {
                let _size = stdout
                    .read(&mut bytesr)
                    .context("failed at reading stdout")?;
                if _size > 0 {
                    let _size = unix_stream
                        .write(&bytesr)
                        .context("Failed at writing the unix stream")?;
                }
                let _size = unix_stream
                    .read(&mut bytes)
                    .context("Failed at reading the unix stream")?;
                if _size > 0 {
                    stdin.write(&bytes).context("failed at writing stdin")?;
                }
            }
        }
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
    for i in io::stdin().bytes() {
        let bytes = vec![i?];
        unix_stream
            .write(&bytes)
            .context("Failed at writing onto the unix stream")?;
        let size = unix_stream
            .read(&mut bytesr)
            .context("Failed at reading the unix stream")?;
        _stdout.write_all(&bytes)?;
        _stdout.flush()?;
    }

    println!("We sent a request");
    println!("Shutting down writing on the stream, waiting for response...");
    let mut bytes = [0; 200];

    let size = unix_stream
        .read(&mut bytes)
        .context("Failed at reading the unix stream")?;

    let message = std::str::from_utf8(&bytes)?;

    println!("We received this message: {}\nReplying...", message);

    unix_stream
        .shutdown(std::net::Shutdown::Write)
        .context("Could not shutdown writing on the stream")?;

    Ok(())
}

fn main() -> anyhow::Result<()> {
    let arg1 = std::env::args().nth(1);
    match arg1 {
        Some(action) if action == "server" => server(),
        Some(action) if action == "client" => client(),
        _ => Ok(()),
    }
}
