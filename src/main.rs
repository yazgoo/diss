use clap::Parser;
use diss::{list_sessions, server_client};

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// list sessions
    #[clap(short, long, value_parser)]
    list: bool,

    // escape key
    #[clap(short, long, value_parser)]
    escape_key: Option<String>,

    // session name
    #[clap(short, long, value_parser)]
    attach_session: Option<String>,

    // command
    command: Vec<String>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    if args.list {
        for session in list_sessions()? {
            println!("{}", session);
        }
    }
    args.attach_session
        .as_ref()
        .map(|session_name| server_client(session_name, &args.command));
    Ok(())
}
