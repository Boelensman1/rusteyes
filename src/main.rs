fn main() {
    if let Err(error) = resteyes::run() {
        eprintln!("resteyes: {error}");
        std::process::exit(1);
    }
}
