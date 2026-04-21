fn main() {
    if let Err(error) = support::run_anon_pipe() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
