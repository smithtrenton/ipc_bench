fn main() {
    if let Err(error) = support::run_named_pipe(support::NamedPipeKind::ByteSync) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
