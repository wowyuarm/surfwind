mod agent;
mod cli;
mod config;
mod models;
mod runstore;
mod runtime;
mod settings;
mod translator;
mod types;

fn main() {
    match cli::run() {
        Ok(code) => std::process::exit(code),
        Err(err) => {
            eprintln!("surfwind error: {err}");
            std::process::exit(1);
        }
    }
}
