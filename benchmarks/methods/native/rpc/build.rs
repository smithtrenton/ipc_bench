use std::{
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    println!("cargo:rerun-if-changed=rpc/ipc_bench_rpc.idl");
    println!("cargo:rerun-if-changed=rpc/rpc_wrapper.c");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR missing"));
    let generated_dir = out_dir.join("generated");
    fs::create_dir_all(&generated_dir).expect("failed to create generated directory");

    let source_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir missing"));
    let rpc_dir = source_dir.join("rpc");
    let idl_path = rpc_dir.join("ipc_bench_rpc.idl");
    let wrapper_path = rpc_dir.join("rpc_wrapper.c");
    let cl = find_msvc_tool("cl.exe").expect("failed to locate cl.exe");
    let lib = find_msvc_tool("lib.exe").expect("failed to locate lib.exe");
    let msvc_include = cl
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(|path| path.join("include"))
        .expect("failed to compute MSVC include directory");
    let sdk_include =
        find_windows_sdk_include_root().expect("failed to locate Windows SDK includes");
    let sdk_shared = sdk_include.join("shared");
    let sdk_um = sdk_include.join("um");
    let sdk_ucrt = sdk_include.join("ucrt");
    let cpp_wrapper = out_dir.join("cl_cpp.cmd");
    fs::write(
        &cpp_wrapper,
        format!("@echo off\r\n\"{}\" %*\r\n", cl.display()),
    )
    .expect("failed to write cl wrapper");

    let midl = find_midl().expect("failed to locate midl.exe");
    run_command(
        Command::new(midl)
            .arg("/nologo")
            .arg("/char")
            .arg("signed")
            .arg("/env")
            .arg("x64")
            .arg("/target")
            .arg("NT100")
            .arg("/cpp_cmd")
            .arg(&cpp_wrapper)
            .arg("/out")
            .arg(&generated_dir)
            .arg(&idl_path),
        "midl",
    );

    let generated_sources = vec![
        generated_dir.join("ipc_bench_rpc_c.c"),
        generated_dir.join("ipc_bench_rpc_s.c"),
        wrapper_path,
    ];

    let mut objects = Vec::new();
    for source in &generated_sources {
        let object = out_dir.join(
            source
                .file_stem()
                .expect("source should have stem")
                .to_string_lossy()
                .replace('.', "_")
                + ".obj",
        );
        let is_client_stub = source
            .file_name()
            .and_then(OsStr::to_str)
            .map(|name| name.eq_ignore_ascii_case("ipc_bench_rpc_c.c"))
            .unwrap_or(false);
        let mut command = Command::new(&cl);
        command
            .arg("/nologo")
            .arg("/c")
            .arg(format!("/I{}", generated_dir.display()))
            .arg(format!("/I{}", rpc_dir.display()))
            .arg(format!("/I{}", msvc_include.display()))
            .arg(format!("/I{}", sdk_shared.display()))
            .arg(format!("/I{}", sdk_um.display()))
            .arg(format!("/I{}", sdk_ucrt.display()))
            .arg("/D_M_AMD64=100")
            .arg("/DWIN32")
            .arg("/D_WIN32")
            .arg("/DWIN64")
            .arg("/D_WIN64");
        if is_client_stub {
            command.arg("/DIpcBenchPing=IpcBenchPingClient");
        }
        command.arg(source).arg(format!("/Fo{}", object.display()));
        run_command(&mut command, "cl");
        objects.push(object);
    }

    let library = out_dir.join("ipc_bench_rpc_native.lib");
    let mut ar = Command::new(lib);
    ar.arg("/nologo").arg(format!("/OUT:{}", library.display()));
    for object in &objects {
        ar.arg(object);
    }
    run_command(&mut ar, "lib");

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=ipc_bench_rpc_native");
    println!("cargo:rustc-link-lib=rpcrt4");
}

fn run_command(command: &mut Command, description: &str) {
    let status = command.status().unwrap_or_else(|error| {
        panic!("failed to launch {description}: {error}");
    });
    if !status.success() {
        panic!("{description} failed with status {status}");
    }
}

fn find_midl() -> Option<PathBuf> {
    if command_exists("midl") {
        return Some(PathBuf::from("midl"));
    }

    let kits = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut candidates = fs::read_dir(kits)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("x64").join("midl.exe"))
        .filter(|path| path.exists())
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.pop()
}

fn find_msvc_tool(name: &str) -> Option<PathBuf> {
    let roots = [
        Path::new(r"C:\Program Files\Microsoft Visual Studio\2022"),
        Path::new(r"C:\Program Files (x86)\Microsoft Visual Studio\2022"),
    ];

    let mut candidates = Vec::new();
    for root in roots {
        if let Ok(editions) = fs::read_dir(root) {
            for edition in editions.filter_map(Result::ok) {
                let tools = edition.path().join("VC").join("Tools").join("MSVC");
                if let Ok(versions) = fs::read_dir(&tools) {
                    for version in versions.filter_map(Result::ok) {
                        let path = version
                            .path()
                            .join("bin")
                            .join("Hostx64")
                            .join("x64")
                            .join(name);
                        if path.exists() {
                            candidates.push(path);
                        }
                    }
                }
            }
        }
    }

    candidates.sort();
    candidates.pop()
}

fn find_windows_sdk_include_root() -> Option<PathBuf> {
    let roots = [
        Path::new(r"C:\Program Files (x86)\Windows Kits\10\Include"),
        Path::new(r"C:\Program Files\Windows Kits\10\Include"),
    ];

    let mut candidates = Vec::new();
    for root in roots {
        if let Ok(entries) = fs::read_dir(root) {
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if path.join("shared").exists()
                    && path.join("um").exists()
                    && path.join("ucrt").exists()
                {
                    candidates.push(path);
                }
            }
        }
    }

    candidates.sort();
    candidates.pop()
}

fn command_exists(command: &str) -> bool {
    Command::new(command)
        .arg("/?")
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}
