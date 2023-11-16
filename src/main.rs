use std::collections::HashMap;

use clap::Parser;
use diss::{list_sessions, run};

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// list sessions
    #[clap(short, long, value_parser)]
    list: bool,

    // debug
    #[clap(short, long, value_parser)]
    debug: bool,

    // escape key
    #[clap(short, long, value_parser)]
    escape_key: Option<String>,

    // session name
    #[clap(short, long, value_parser)]
    attach_session: Option<String>,

    // kill session
    #[clap(short, long, value_parser)]
    kill: Option<String>,

    // command
    command: Vec<String>,
}

fn setup_logger() -> Result<(), fern::InitError> {
    fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "[{}][{}] {}",
                record.target(),
                record.level(),
                message
            ))
        })
        .level(log::LevelFilter::Debug)
        .chain(fern::log_file("diss.log")?)
        .apply()?;
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    if args.debug {
        setup_logger()?;
    }
    if args.list {
        for session in list_sessions()? {
            println!("{}", session);
        }
    }
    if args.kill.is_some() {
        return args
            .kill
            .as_ref()
            .map(|session_name| diss::kill_session(session_name))
            .unwrap();
    }
    let env = HashMap::new();
    args.attach_session
        .as_ref()
        .map(|session_name| run(session_name, &args.command, env, args.escape_key.clone()));
    Ok(())
}
