#![cfg(windows)]
#![allow(dead_code)]

mod agent;
mod app;
mod child;
mod conpty;
mod core;
mod driver;
mod pipe;
mod proc;
mod sd;
mod svc;
mod util;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("prove");
    let rest = &args[1.min(args.len())..];
    match mode {
        "service" => svc::dispatch(),
        "core" => core::run(rest),
        "app" => app::run(rest),
        "agent" => agent::run(),
        "child" => child::run(),
        "second" => driver::second(),
        _ => driver::run(),
    }
}
