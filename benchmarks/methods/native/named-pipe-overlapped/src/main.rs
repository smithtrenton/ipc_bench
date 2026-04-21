fn main() {
    if let Err(error) = support::run_named_pipe(support::NamedPipeKind::Overlapped) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
