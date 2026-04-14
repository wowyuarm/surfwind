fn main() {
    match surfwind::cli::run() {
        Ok(code) => std::process::exit(code),
        Err(err) => {
            eprintln!("surfwind error: {err}");
            std::process::exit(1);
        }
    }
}
