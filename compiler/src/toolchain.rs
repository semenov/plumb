// The tools plumb fetches for itself the first time they're needed, pinned
// to versions it works with and checked against their SHA-256:
//
// - wasi-sdk (clang for WebAssembly), for `--target wasm-server`;
// - wasm-server, for `plumb dev`.
//
// They live in $PLUMB_HOME (~/.plumb by default): toolchains/wasi-sdk-34.0/,
// bin/wasm-server-v0.2.2. A tool already on the machine is used instead:
// WASI_SDK, /opt/wasi-sdk or ~/wasi-sdk; WASM_SERVER or `wasm-server` on
// the PATH.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const WASI_SDK_VERSION: &str = "34.0";
/// (asset, SHA-256), from the release's published digests.
const WASI_SDK_ASSETS: [(&str, &str, &str); 4] = [
    ("macos", "aarch64", "wasi-sdk-34.0-arm64-macos.tar.gz:9c59398106b417f8f14913380fdf0097a8cc0ff4af9eb3ce0065a859e88d49e9"),
    ("macos", "x86_64", "wasi-sdk-34.0-x86_64-macos.tar.gz:87d27fa8adc68dee59bfbf2e22a6d34ef717c34d6bf1d8af2a56fc929d9ce0eb"),
    ("linux", "aarch64", "wasi-sdk-34.0-arm64-linux.tar.gz:f7e243dff54d60bcc576e94d6166b69f410f2500ae4a9ceef34315be10e77971"),
    ("linux", "x86_64", "wasi-sdk-34.0-x86_64-linux.tar.gz:b761e3a0721dbae9c09a0059e5fdb2bf917d1b4a8a7b430fb3b5aafb0984b2c4"),
];
/// The wasm-server release `plumb dev` runs (its interfaces match this
/// compiler's).
pub const WASM_SERVER_VERSION: &str = "v0.2.2";

pub fn home() -> PathBuf {
    if let Ok(h) = std::env::var("PLUMB_HOME") {
        return PathBuf::from(h);
    }
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".plumb")
}

fn host() -> Option<(&'static str, &'static str)> {
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        return None;
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else {
        return None;
    };
    Some((os, arch))
}

/// Downloads `url` to `out`, showing progress; then checks its SHA-256.
fn fetch(url: &str, out: &Path, sha256: &str) -> Result<(), String> {
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {}", d.display(), e))?;
    }
    let ok = Command::new("curl").args(["-fL", "--progress-bar", "-o"]).arg(out).arg(url).stdin(Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
    if !ok {
        let _ = std::fs::remove_file(out);
        return Err(format!("can't download {}", url));
    }
    let data = std::fs::read(out).map_err(|e| e.to_string())?;
    let actual = crate::deploy::sha256_hex(&data);
    if actual != sha256 {
        let _ = std::fs::remove_file(out);
        return Err(format!("{} doesn't match its checksum (got {}, expected {})", url, actual, sha256));
    }
    Ok(())
}

/// The wasi-sdk to build with: one on this machine, or the pinned one,
/// downloaded the first time.
pub fn wasi_sdk() -> Result<PathBuf, String> {
    let mut places: Vec<PathBuf> = vec![];
    if let Ok(p) = std::env::var("WASI_SDK") {
        places.push(PathBuf::from(p));
    }
    places.push(PathBuf::from("/opt/wasi-sdk"));
    if let Ok(h) = std::env::var("HOME") {
        places.push(PathBuf::from(h).join("wasi-sdk"));
    }
    let ours = home().join("toolchains").join(format!("wasi-sdk-{}", WASI_SDK_VERSION));
    places.push(ours.clone());
    if let Some(p) = places.into_iter().find(|p| p.join("bin/clang").exists()) {
        return Ok(p);
    }
    let (os, arch) = host().ok_or("there's no wasi-sdk for this system; set WASI_SDK to one")?;
    let (asset, sha) = WASI_SDK_ASSETS.iter().find(|(o, a, _)| *o == os && *a == arch).and_then(|(_, _, s)| s.split_once(':')).ok_or("there's no wasi-sdk for this system; set WASI_SDK to one")?;
    eprintln!("plumb: building for wasm-server needs wasi-sdk {}: downloading it once (about 180 MB) into {}", WASI_SDK_VERSION, ours.display());
    let tmp = home().join("toolchains").join(format!(".download-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let archive = tmp.join(asset);
    let url = format!("https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-{}/{}", WASI_SDK_VERSION.split('.').next().unwrap_or("34"), asset);
    let result = (|| -> Result<(), String> {
        fetch(&url, &archive, sha)?;
        let ok = Command::new("tar").arg("-xzf").arg(&archive).arg("-C").arg(&tmp).status().map(|s| s.success()).unwrap_or(false);
        if !ok {
            return Err(format!("can't unpack {}", archive.display()));
        }
        // the archive holds one directory, wasi-sdk-34.0-arm64-macos
        let unpacked = std::fs::read_dir(&tmp).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).find(|p| p.join("bin/clang").exists()).ok_or("the archive has no bin/clang")?;
        std::fs::rename(&unpacked, &ours).map_err(|e| format!("{}: {}", ours.display(), e))
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result?;
    eprintln!("plumb: wasi-sdk is in {}", ours.display());
    Ok(ours)
}

/// The wasm-server `plumb dev` runs: WASM_SERVER, one on the PATH, or the
/// pinned release, downloaded the first time.
pub fn wasm_server() -> Result<PathBuf, String> {
    if let Ok(p) = std::env::var("WASM_SERVER") {
        return Ok(PathBuf::from(p));
    }
    if let Some(p) = std::env::var("PATH").ok().and_then(|path| path.split(':').map(|d| Path::new(d).join("wasm-server")).find(|p| p.exists())) {
        return Ok(p);
    }
    let ours = home().join("bin").join(format!("wasm-server-{}", WASM_SERVER_VERSION));
    if ours.exists() {
        return Ok(ours);
    }
    let (os, arch) = host().ok_or("there's no wasm-server build for this system; set WASM_SERVER to one")?;
    let asset = format!("wasm-server-{}-{}.gz", os, arch);
    let base = format!("https://github.com/semenov/wasm-server/releases/download/{}", WASM_SERVER_VERSION);
    eprintln!("plumb: plumb dev runs wasm-server {}: downloading it once into {}", WASM_SERVER_VERSION, ours.display());
    // its checksum, from the release
    let sums = Command::new("curl").args(["-fsSL", &format!("{}/SHA256SUMS", base)]).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).to_string()).ok_or("can't download wasm-server's checksums")?;
    let sha = sums.lines().find_map(|l| l.strip_suffix(&format!("  {}", asset)).map(str::to_string)).ok_or_else(|| format!("wasm-server {} has no build for {} {}", WASM_SERVER_VERSION, os, arch))?;
    let gz = ours.with_extension("gz.download");
    fetch(&format!("{}/{}", base, asset), &gz, sha.trim())?;
    let tmp = ours.with_extension("download");
    let out = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    let ok = Command::new("gunzip").arg("-c").arg(&gz).stdout(out).status().map(|s| s.success()).unwrap_or(false);
    let _ = std::fs::remove_file(&gz);
    if !ok {
        return Err("can't unpack wasm-server".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
    }
    std::fs::rename(&tmp, &ours).map_err(|e| e.to_string())?;
    Ok(ours)
}
