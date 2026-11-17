fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("windows") {
        return;
    }
    println!("cargo:rerun-if-changed=app.manifest");
    let manifest = std::env::current_dir()
        .expect("package directory")
        .join("app.manifest");
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let rc_path = out_dir.join("desktop_test_manifest.rc");
    let res_path = out_dir.join("desktop_test_manifest.res");
    let obj_path = out_dir.join("desktop_test_manifest.obj");
    let lib_path = out_dir.join("desktop_test_manifest.lib");
    let manifest_rc = manifest.display().to_string().replace('\\', "\\\\");
    std::fs::write(&rc_path, format!("1 24 \"{manifest_rc}\"\n")).expect("write resource script");
    let machine = if target.contains("aarch64") {
        "arm64"
    } else if target.contains("i686") {
        "x86"
    } else {
        "x64"
    };
    run(
        &find_rc(),
        &[
            "/nologo".to_string(),
            format!("/fo{}", res_path.display()),
            rc_path.display().to_string(),
        ],
    );
    let msvc = find_msvc_bin();
    run(
        &msvc.join("cvtres.exe"),
        &[
            "/nologo".to_string(),
            format!("/machine:{machine}"),
            format!("/out:{}", obj_path.display()),
            res_path.display().to_string(),
        ],
    );
    run(
        &msvc.join("lib.exe"),
        &[
            "/nologo".to_string(),
            format!("/machine:{machine}"),
            format!("/out:{}", lib_path.display()),
            obj_path.display().to_string(),
        ],
    );
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static:+whole-archive=desktop_test_manifest");
}

fn run(program: &std::path::Path, args: &[String]) {
    let status = std::process::Command::new(program)
        .args(args)
        .status()
        .unwrap_or_else(|error| panic!("failed to run {}: {error}", program.display()));
    if !status.success() {
        panic!(
            "{} failed to embed the Windows test manifest",
            program.display()
        );
    }
}

fn find_rc() -> std::path::PathBuf {
    let kits = std::path::Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut found = Vec::new();
    if let Ok(entries) = std::fs::read_dir(kits) {
        for entry in entries.flatten() {
            let rc = entry.path().join(r"x64\rc.exe");
            if rc.is_file() {
                found.push(rc);
            }
        }
    }
    found.sort();
    found.pop().expect("Windows SDK rc.exe")
}

fn find_msvc_bin() -> std::path::PathBuf {
    let mut found = Vec::new();
    for root in [
        r"C:\Program Files (x86)\Microsoft Visual Studio",
        r"C:\Program Files\Microsoft Visual Studio",
    ] {
        let Ok(years) = std::fs::read_dir(root) else {
            continue;
        };
        for year in years.flatten() {
            let Ok(products) = std::fs::read_dir(year.path()) else {
                continue;
            };
            for product in products.flatten() {
                let msvc = product.path().join(r"VC\Tools\MSVC");
                let Ok(versions) = std::fs::read_dir(msvc) else {
                    continue;
                };
                for version in versions.flatten() {
                    let bin = version.path().join(r"bin\Hostx64\x64");
                    if bin.join("lib.exe").is_file() && bin.join("cvtres.exe").is_file() {
                        found.push(bin);
                    }
                }
            }
        }
    }
    found.sort();
    found.pop().expect("MSVC lib.exe and cvtres.exe")
}
