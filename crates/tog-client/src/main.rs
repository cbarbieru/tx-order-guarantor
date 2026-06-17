//! CLI for the enclave protocol.
//!
//!   tog-client [--addr H:P] <command>                       # plaintext (dev)
//!   tog-client [--addr H:P] --ratls --ca CA.pem [--server-name CN] <command>
//!
//! Commands:
//!   send <0xRAWTX>   submit a raw transaction, print its hash
//!   get-raw          drain + print the raw tx buffer
//!   get-best         print the enclave-computed ordering
//!   demo             send sample txs (distinct tips), then show the ordering
//!
//! `--ratls` requires building with `--features ratls`.

use std::error::Error;
use std::io::{Read, Write};

use tog_client::{connect_plain, sample_raw_tx, Client};

struct Args {
    addr: String,
    ratls: bool,
    // Read only under `--features ratls`; harmless dead fields otherwise.
    #[cfg_attr(not(feature = "ratls"), allow(dead_code))]
    ca: Option<String>,
    #[cfg_attr(not(feature = "ratls"), allow(dead_code))]
    server_name: Option<String>,
    rest: Vec<String>,
}

fn parse_args() -> Args {
    let mut addr = "127.0.0.1:1546".to_string();
    let mut ratls = false;
    let mut ca = None;
    let mut server_name = None;
    let mut rest = Vec::new();

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--addr" => addr = it.next().unwrap_or(addr),
            "--ratls" => ratls = true,
            "--ca" => ca = it.next(),
            "--server-name" => server_name = it.next(),
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            _ => rest.push(arg),
        }
    }
    Args { addr, ratls, ca, server_name, rest }
}

fn print_usage() {
    eprintln!(
        "usage: tog-client [--addr H:P] [--ratls --ca CA.pem [--server-name CN]] <command>\n\
         commands: send <0xRAWTX> | get-raw | get-best | demo"
    );
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = parse_args();

    if args.ratls {
        connect_ratls_and_run(&args)
    } else {
        let mut client = connect_plain(&args.addr)?;
        eprintln!("connected (plaintext) to {}", args.addr);
        run(&mut client, &args.rest)
    }
}

#[cfg(feature = "ratls")]
fn connect_ratls_and_run(args: &Args) -> Result<(), Box<dyn Error>> {
    let ca = args.ca.as_deref().ok_or("--ratls requires --ca <CA.pem>")?;
    let mut client = tog_client::connect_ratls(&args.addr, ca, args.server_name.as_deref())?;
    eprintln!("connected (RA-TLS, verified against {ca}) to {}", args.addr);
    run(&mut client, &args.rest)
}

#[cfg(not(feature = "ratls"))]
fn connect_ratls_and_run(_args: &Args) -> Result<(), Box<dyn Error>> {
    Err("this binary was built without RA-TLS; rebuild with `--features ratls`".into())
}

fn run<S: Read + Write>(client: &mut Client<S>, cmd: &[String]) -> Result<(), Box<dyn Error>> {
    match cmd.first().map(String::as_str) {
        Some("send") => {
            let raw = cmd.get(1).ok_or("send requires a 0x-prefixed raw tx")?;
            println!("{}", client.send_raw_transaction(raw)?);
        }
        Some("get-raw") => {
            for t in client.get_raw_transactions()? {
                println!("{t}");
            }
        }
        Some("get-best") => {
            for h in client.get_best_transaction_hashes()? {
                println!("{h}");
            }
        }
        Some("demo") => run_demo(client)?,
        _ => {
            print_usage();
            return Err("no command".into());
        }
    }
    Ok(())
}

fn run_demo<S: Read + Write>(client: &mut Client<S>) -> Result<(), Box<dyn Error>> {
    // Three distinct senders with different tips; expected best order is
    // tip-descending: 100, 50, 5.
    let samples = [(0x11u8, 0u64, 5u128), (0x22, 0, 100), (0x33, 0, 50)];
    println!("→ sending {} sample txs", samples.len());
    let mut sent = Vec::new();
    for (key, nonce, tip) in samples {
        let raw = sample_raw_tx(key, nonce, tip);
        let hash = client.send_raw_transaction(&raw)?;
        println!("  tip={tip:>4}  {hash}");
        sent.push((tip, hash));
    }

    println!("\n→ get-best (expect tip-desc 100, 50, 5):");
    for h in client.get_best_transaction_hashes()? {
        let tip = sent.iter().find(|(_, hh)| *hh == h).map(|(t, _)| *t);
        match tip {
            Some(t) => println!("  {h}  (tip={t})"),
            None => println!("  {h}"),
        }
    }

    println!("\n→ get-raw (drains the buffer):");
    let raws = client.get_raw_transactions()?;
    println!("  {} raw txs returned", raws.len());
    println!("  (second call should be empty:)");
    println!("  {} raw txs returned", client.get_raw_transactions()?.len());
    Ok(())
}
