use std::{
    ffi::{OsStr, OsString},
    io::{self, BufRead, BufReader, Read},
    process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio},
};

pub struct ManagedChild {
    child: Child,
    stdout: BufReader<ChildStdout>,
    stdin: Option<ChildStdin>,
}

impl ManagedChild {
    pub fn spawn_self(args: &[OsString]) -> io::Result<Self> {
        let envs: [(OsString, OsString); 0] = [];
        Self::spawn_self_with_env(args, &envs)
    }

    pub fn spawn_self_with_env<K, V>(args: &[OsString], envs: &[(K, V)]) -> io::Result<Self>
    where
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        let executable = std::env::current_exe()?;
        let mut command = Command::new(executable);
        command
            .args(args)
            .envs(
                envs.iter()
                    .map(|(key, value)| (key.as_ref(), value.as_ref())),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        let mut child = command.spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("failed to capture child stdout"))?;
        let stdin = child.stdin.take();

        Ok(Self {
            child,
            stdout: BufReader::new(stdout),
            stdin,
        })
    }

    pub fn wait_for_ready(&mut self) -> io::Result<String> {
        let mut line = String::new();
        let bytes_read = self.stdout.read_line(&mut line)?;
        if bytes_read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "child exited before signaling readiness",
            ));
        }

        Ok(line.trim().to_owned())
    }

    pub fn request_shutdown(&mut self) {
        self.stdin.take();
    }

    pub fn wait(mut self) -> io::Result<ExitStatus> {
        self.stdin.take();
        self.child.wait()
    }
}

pub fn hold_until_stdin_closes() -> io::Result<()> {
    let mut sink = Vec::new();
    io::stdin().read_to_end(&mut sink).map(|_| ())
}
