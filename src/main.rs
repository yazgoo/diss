use std::collections::HashMap;

use clap::Parser;
use diss::{list_sessions, run};

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
    let env = HashMap::new();
    args.attach_session
        .as_ref()
        .map(|session_name| run(session_name, &args.command, env, args.escape_key.clone()));
    Ok(())
}
