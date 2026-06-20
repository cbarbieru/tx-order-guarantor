//! CLI for the enclave protocol.
//!
//!   tog-client [--addr H:P] <command>       # plaintext
//!
//! Commands:
//!   send <0xRAWTX>   submit a raw transaction, print its hash
//!   get-raw          drain + print the raw tx buffer
//!   get-best         print the enclave-computed ordering
//!   demo             send sample txs (distinct tips), then show the ordering
//!
//! The enclave always opens with a dev STUB attestation (fake — proves nothing);
//! the client reads and surfaces it before running the command.

use std::error::Error;
use std::io::{Read, Write};

use tog_client::{Client, connect_plain, sample_raw_tx};

struct Args {
    addr: String,
    rest: Vec<String>,
}

fn parse_args() -> Args {
    let mut addr = "127.0.0.1:1546".to_string();
    let mut rest = Vec::new();

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--addr" => addr = it.next().unwrap_or(addr),
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            _ => rest.push(arg),
        }
    }
    Args { addr, rest }
}

fn print_usage() {
    eprintln!(
        "usage: tog-client [--addr H:P] <command>\n\
         commands: send <0xRAWTX> | get-raw | get-best | demo"
    );
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = parse_args();

    let mut client = connect_plain(&args.addr)?;
    eprintln!("connected (plaintext) to {}", args.addr);

    // The enclave always opens with a stub attestation frame — read + surface it.
    match client.read_stub_attestation() {
        Ok(att) if att.stub => {
            eprintln!("🔒 attestation: STUB (dev) — peer claims SGX identity:");
            eprintln!("     mr_enclave = {}", att.mr_enclave);
            eprintln!("     mr_signer  = {}", att.mr_signer);
            eprintln!("     ⚠ {}", att.note);
        }
        Ok(_) => return Err("peer sent a non-stub attestation".into()),
        Err(e) => return Err(format!("expected a stub attestation frame first ({e})").into()),
    }

    run(&mut client, &args.rest)
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
