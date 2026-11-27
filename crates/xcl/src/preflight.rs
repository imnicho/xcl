//! What the PC has of what a run needs, checked by `xcl run` before the game starts: DXC, the
//! system XGameRuntime, the GPU the game will get (Shader Model 6.6, mesh shaders, wave
//! width) and room on the drives a run writes to. Without one of them the game still starts,
//! but draws nothing or part of the picture, so each missing piece is printed with what to do,
//! and kept in `report.json`.

use serde::{Deserialize, Serialize};

/// How much a missing piece costs the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Ok,
    /// runs, with a known limit
    Note,
    /// part of the game is missing (no geometry, no system services, software rendering)
    Warning,
    /// the game draws nothing
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub level: Level,
    pub what: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

/// The GPU the game gets, as D3D12 reports it.
#[derive(Debug, Clone, Default)]
pub struct Gpu {
    pub name: String,
    pub warp: bool,
    /// (6, 6) = Shader Model 6.6
    pub shader_model: (u8, u8),
    /// the message for a missing SM 6.6 (it depends on the Agility SDK state)
    pub no_sm66: String,
    pub mesh: bool,
    pub wave_min: u32,
    pub wave_max: u32,
}

/// Everything the checks look at; `checks` turns it into the list.
#[derive(Debug, Clone)]
pub struct Found {
    pub dxc: Result<String, String>,
    /// the DXC in that folder, for its version
    pub dxc_toolchain: crate::gpu::recomp::dxc::Toolchain,
    pub xgameruntime: bool,
    pub gpu: Result<Gpu, String>,
    /// the drives a run writes to (`drives`)
    pub drives: Vec<Drive>,
}

/// A drive a run writes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drive {
    /// its root, `C:\`
    pub root: String,
    /// bytes free for this user
    pub free: u64,
    /// what a run writes there
    pub holds: Vec<&'static str>,
}

/// Free space under which a drive a run writes to is reported: 4 GiB. The GPU driver keeps a
/// shader cache of its own on the drive of the user's profile, whatever `XCL_DATA_ROOT` and
/// `XCL_DXIL_CACHE` say (NVIDIA: `%LOCALAPPDATA%\NVIDIA\DXCache`), and it grows with the
/// pipelines a title makes: on an RTX 5080 (driver 610.47) the files of `xcl.exe` there reached
/// 34.6 GB in two days of runs of a retail title, in ten files of growing size (the largest
/// 8 and 16 GB), and the drive was full in the middle of a run. Nothing a run does says so.
pub const DRIVE_FREE_LEAST: u64 = 4 << 30;

const DRIVE_FIX: &str = r#"free space on it; on NVIDIA the driver's own shader cache (%LOCALAPPDATA%\NVIDIA\DXCache) grows with every pipeline a title makes, and "Shader Cache Size" in the NVIDIA Control Panel caps it"#;

/// The root of the drive `path` is on (`C:\`, `\\server\share\`); None for a path without one.
fn drive_root(path: &std::path::Path) -> Option<String> {
    use std::path::{Component, Prefix};
    let Component::Prefix(p) = path.components().next()? else { return None };
    match p.kind() {
        Prefix::Disk(d) | Prefix::VerbatimDisk(d) => Some(format!(r"{}:\", (d as char).to_ascii_uppercase())),
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => Some(format!(r"\\{}\{}\", server.to_string_lossy(), share.to_string_lossy())),
        _ => None,
    }
}

/// Bytes free for this user on the drive with the root `root`.
fn free_bytes(root: &str) -> Option<u64> {
    let mut free = 0u64;
    // SAFETY: a NUL-terminated path and one valid out-pointer; the other two are optional.
    let ok = unsafe { windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(crate::util::wide(root).as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
    (ok != 0).then_some(free)
}

/// The drives of `folders` (what a run writes, and where), each once with everything it holds,
/// in the order they come; `free` gives a root's free bytes. A folder without a drive, or on
/// one that does not answer, is left out.
pub fn drives(folders: &[(&'static str, std::path::PathBuf)], free: impl Fn(&str) -> Option<u64>) -> Vec<Drive> {
    let mut v: Vec<Drive> = Vec::new();
    for (what, folder) in folders {
        let Some(root) = drive_root(folder) else { continue };
        match v.iter_mut().find(|d| d.root.eq_ignore_ascii_case(&root)) {
            Some(d) => d.holds.push(*what),
            None => {
                if let Some(bytes) = free(&root) {
                    v.push(Drive { root, free: bytes, holds: vec![*what] });
                }
            }
        }
    }
    v
}

/// What a run writes and where: XCL's data (logs, saves, a title's settings), XCL's shader
/// cache, and the GPU driver's own shader cache, which is under the user's profile.
fn written_folders() -> Vec<(&'static str, std::path::PathBuf)> {
    let mut v = vec![("XCL's data and logs", crate::launcher::data_root()), ("XCL's shader cache", crate::gpu::recomp::dxc::cache_dir())];
    if let Some(local) = std::env::var_os("LOCALAPPDATA").filter(|p| !p.is_empty()) {
        v.push(("the GPU driver's own shader cache", local.into()));
    }
    v
}

fn check(level: Level, what: impl Into<String>, fix: Option<&str>) -> Check {
    Check { level, what: what.into(), fix: fix.map(str::to_string) }
}

pub fn checks(f: &Found) -> Vec<Check> {
    let mut v = Vec::new();
    match &f.gpu {
        Err(e) => v.push(check(Level::Error, format!("no D3D12 device: {e}"), Some("update the GPU driver"))),
        Ok(g) => {
            if g.warp {
                v.push(check(Level::Warning, "GPU: WARP (software renderer) – no hardware D3D12 device was usable", Some("update the GPU driver; d3d12_adapter in xcl.toml or XCL_ADAPTER picks the GPU")));
            } else {
                v.push(check(Level::Ok, format!("GPU: {}", g.name), None));
            }
            if g.shader_model < (6, 6) {
                v.push(check(Level::Error, format!("{} (has {}.{}): shaders cannot run", g.no_sm66, g.shader_model.0, g.shader_model.1), Some("update the GPU driver; on Windows 10, put the D3D12 Agility SDK's D3D12Core.dll in a D3D12 folder next to xcl.exe")));
            }
            if !g.mesh {
                v.push(check(Level::Warning, "no mesh shaders: the game's geometry and NGG programs are not drawn", Some("a GPU with D3D12 mesh shaders is needed (NVIDIA RTX 20 / GTX 16 or newer, AMD RX 6000 or newer, Intel Arc)")));
            }
            let wave64 = g.wave_min <= 64 && g.wave_max >= 64;
            let wave32 = g.wave_min <= 32 && g.wave_max >= 32;
            if !wave64 && wave32 {
                v.push(check(Level::Note, format!("waves of {}–{} lanes: the console's 64-lane programs run as two 32-lane halves (slower; vertex / pixel shaders that read lanes 32–63 can differ)", g.wave_min, g.wave_max), None));
            } else if !wave64 && !wave32 && g.wave_max > 0 {
                v.push(check(Level::Error, format!("waves of {}–{} lanes: neither 64 nor 32, the console's programs cannot run", g.wave_min, g.wave_max), None));
            }
        }
    }
    match &f.dxc {
        Ok(dir) => {
            use crate::gpu::recomp::dxc;
            let t = &f.dxc_toolchain;
            v.push(check(Level::Ok, format!("DXC {}: {dir}", t.dxcompiler), None));
            if t.outdated() {
                v.push(check(Level::Note, format!("DXC is older than {}.{}: {}", dxc::RECOMMENDED.0, dxc::RECOMMENDED.1, dxc::OUTDATED_COST), Some(dxc::OUTDATED_FIX)));
            }
        }
        Err(_) => v.push(check(Level::Error, "no DXC (dxcompiler.dll + dxil.dll): no shader can be compiled, the game draws nothing", Some("install the Windows SDK, or put dxcompiler.dll and dxil.dll next to xcl.exe (or in %XCL_DXC_DIR%)"))),
    }
    for d in f.drives.iter().filter(|d| d.free < DRIVE_FREE_LEAST) {
        v.push(check(Level::Warning, format!("drive {} has {} MB free, and a run writes there: {}", d.root, d.free >> 20, d.holds.join(", ")), Some(DRIVE_FIX)));
    }
    if !f.xgameruntime {
        v.push(check(Level::Warning, "no System32\\xgameruntime.dll: dialogs, text entry, game invites and game streaming are missing (XCL's own task queues, async and networking are used)", Some("install Gaming Services (Microsoft Store)")));
    }
    v
}

/// The adapter `setting` (`d3d12_adapter`, overridden by `XCL_ADAPTER`) picks – the same
/// choice and the same fallback to WARP as the D3D12 backend's device.
fn probe_gpu(setting: &str) -> Result<Gpu, String> {
    use crate::gpu::adapter::{self, Selector};
    use windows::Win32::Graphics::Direct3D::*;
    use windows::Win32::Graphics::Direct3D12::*;
    use windows::Win32::Graphics::Dxgi::*;
    unsafe {
        let factory: IDXGIFactory6 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).map_err(|e| format!("DXGI: {e}"))?;
        let sel = Selector::from_env_or(setting);
        let mut found: Option<(ID3D12Device, String)> = None;
        if sel != Selector::Warp {
            let mut list = adapter::candidates(&factory, &sel);
            if list.is_empty() {
                list = adapter::candidates(&factory, &Selector::Auto);
            }
            for (a, desc) in list {
                if let Ok(dev) = crate::gpu::agility::create_device(&a, D3D_FEATURE_LEVEL_12_0) {
                    found = Some((dev, format!("{} ({} MiB VRAM)", adapter::name(&desc), desc.DedicatedVideoMemory >> 20)));
                    break;
                }
            }
        }
        let warp = found.is_none();
        let (dev, name) = match found {
            Some(x) => x,
            None => {
                let w: IDXGIAdapter = factory.EnumWarpAdapter().map_err(|e| format!("WARP: {e}"))?;
                let d = crate::gpu::agility::create_device(&w, D3D_FEATURE_LEVEL_11_0).map_err(|e| format!("WARP: {e}"))?;
                (d, "WARP (software renderer)".into())
            }
        };
        // the highest model asked for that the runtime knows; a lower one comes back if that is all
        let mut shader_model = (0, 0);
        for m in [D3D_SHADER_MODEL_6_6, D3D_SHADER_MODEL_6_5, D3D_SHADER_MODEL_6_0] {
            let mut sm = D3D12_FEATURE_DATA_SHADER_MODEL { HighestShaderModel: m };
            if dev.CheckFeatureSupport(D3D12_FEATURE_SHADER_MODEL, &mut sm as *mut _ as _, std::mem::size_of_val(&sm) as u32).is_ok() {
                let x = sm.HighestShaderModel.0 as u32;
                shader_model = ((x >> 4) as u8, (x & 15) as u8);
                break;
            }
        }
        let mut o1 = D3D12_FEATURE_DATA_D3D12_OPTIONS1::default();
        let _ = dev.CheckFeatureSupport(D3D12_FEATURE_D3D12_OPTIONS1, &mut o1 as *mut _ as _, std::mem::size_of_val(&o1) as u32);
        let mut o7 = D3D12_FEATURE_DATA_D3D12_OPTIONS7::default();
        let mesh = dev.CheckFeatureSupport(D3D12_FEATURE_D3D12_OPTIONS7, &mut o7 as *mut _ as _, std::mem::size_of_val(&o7) as u32).is_ok()
            && o7.MeshShaderTier.0 >= D3D12_MESH_SHADER_TIER_1.0;
        let no_sm66 = crate::gpu::agility::format_no_sm66_message(&name, crate::gpu::agility::state());
        Ok(Gpu { name, warp, shader_model, no_sm66, mesh, wave_min: o1.WaveLaneCountMin, wave_max: o1.WaveLaneCountMax })
    }
}

fn xgameruntime_present() -> bool {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    std::path::Path::new(&root).join("System32").join("xgameruntime.dll").is_file()
}

/// Check this PC for a run with the adapter `setting`.
pub fn run(setting: &str) -> Vec<Check> {
    let found = Found {
        dxc: crate::gpu::recomp::dxc::location().map(|p| p.display().to_string()),
        dxc_toolchain: crate::gpu::recomp::dxc::toolchain(),
        xgameruntime: xgameruntime_present(),
        gpu: probe_gpu(setting),
        drives: drives(&written_folders(), free_bytes),
    };
    checks(&found)
}

/// The checks as console lines: OK ones first as `  label`, the rest as WARNING / ERROR with
/// what to do.
pub fn print(checks: &[Check]) {
    for c in checks {
        let tag = match c.level {
            Level::Ok => "  ",
            Level::Note => "  NOTE: ",
            Level::Warning => "  WARNING: ",
            Level::Error => "  ERROR: ",
        };
        println!("{tag}{}", c.what);
        if let Some(fix) = &c.fix {
            println!("           → {fix}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gpu() -> Gpu {
        Gpu { name: "Radeon RX 9070 (16304 MiB VRAM)".into(), warp: false, shader_model: (6, 6), no_sm66: "x: no SM 6.6".into(), mesh: true, wave_min: 32, wave_max: 64 }
    }

    fn found(gpu: Gpu) -> Found {
        Found { dxc: Ok(r"C:\dxc".into()), dxc_toolchain: dxc(1, 9), xgameruntime: true, gpu: Ok(gpu), drives: Vec::new() }
    }

    fn dxc(major: u32, minor: u32) -> crate::gpu::recomp::dxc::Toolchain {
        let dxcompiler = crate::gpu::recomp::dxc::Library { major, minor, ..Default::default() };
        crate::gpu::recomp::dxc::Toolchain { dir: r"C:\dxc".into(), dxcompiler, ..Default::default() }
    }

    fn levels(f: &Found) -> Vec<Level> {
        checks(f).into_iter().map(|c| c.level).collect()
    }

    #[test]
    fn a_complete_pc_has_nothing_to_report() {
        assert_eq!(levels(&found(gpu())), [Level::Ok, Level::Ok]);
    }

    #[test]
    fn each_missing_piece_is_reported_with_a_fix() {
        let mut f = found(Gpu { shader_model: (6, 5), mesh: false, ..gpu() });
        f.dxc = Err("dxcompiler.dll not found".into());
        f.xgameruntime = false;
        let c = checks(&f);
        assert_eq!(c.iter().map(|c| c.level).collect::<Vec<_>>(), [Level::Ok, Level::Error, Level::Warning, Level::Error, Level::Warning]);
        assert!(c[1].what.contains("has 6.5"), "{:?}", c[1]);
        // XCL's own task queues and networking stand in for the PC runtime's: not missing
        let missing = c[4].what.split(" missing").next().unwrap();
        assert!(missing.contains("dialogs") && !missing.contains("task queues") && !missing.contains("networking"), "{:?}", c[4]);
        assert!(c.iter().filter(|c| c.level != Level::Ok).all(|c| c.fix.is_some()), "{c:?}");
    }

    #[test]
    fn wave_width() {
        // NVIDIA: 32 only – the split form, a note
        let c = checks(&found(Gpu { wave_min: 32, wave_max: 32, ..gpu() }));
        assert_eq!(c[1].level, Level::Note);
        assert!(c[1].what.contains("two 32-lane halves"), "{:?}", c[1]);
        // 64 in range: nothing; 8–16 (neither size): an error
        assert_eq!(levels(&found(Gpu { wave_min: 64, wave_max: 64, ..gpu() })), [Level::Ok, Level::Ok]);
        assert_eq!(levels(&found(Gpu { wave_min: 8, wave_max: 16, ..gpu() }))[1], Level::Error);
    }

    #[test]
    fn an_old_dxc_is_a_note_with_a_fix() {
        let c = checks(&Found { dxc_toolchain: dxc(1, 7), ..found(gpu()) });
        assert_eq!(c.iter().map(|c| c.level).collect::<Vec<_>>(), [Level::Ok, Level::Ok, Level::Note]);
        assert_eq!(c[1].what, r"DXC 1.7: C:\dxc");
        assert!(c[2].what.contains("older than 1.8") && c[2].fix.as_deref().is_some_and(|f| f.contains("XCL_DXC_DIR")), "{:?}", c[2]);
        assert_eq!(levels(&Found { dxc_toolchain: dxc(1, 8), ..found(gpu()) }), [Level::Ok, Level::Ok]);
    }

    #[test]
    fn warp_and_no_device() {
        assert_eq!(levels(&found(Gpu { warp: true, ..gpu() }))[0], Level::Warning);
        let f = Found { gpu: Err("DXGI: E_FAIL".into()), ..found(gpu()) };
        assert_eq!(checks(&f)[0].level, Level::Error);
    }

    /// A drive a run writes to with under 4 GiB free is named with what the run writes there and
    /// what to do; one with room is not. (The GPU driver's own shader cache filled a system
    /// drive in the middle of a run, and nothing said so.)
    #[test]
    fn a_nearly_full_drive_a_run_writes_to_is_reported() {
        let low = Drive { root: r"C:\".into(), free: DRIVE_FREE_LEAST - 1, holds: vec!["XCL's data and logs", "the GPU driver's own shader cache"] };
        let room = Drive { root: r"D:\".into(), free: DRIVE_FREE_LEAST, holds: vec!["XCL's shader cache"] };
        let c = checks(&Found { drives: vec![low, room], ..found(gpu()) });
        assert_eq!(c.iter().map(|c| c.level).collect::<Vec<_>>(), [Level::Ok, Level::Ok, Level::Warning]);
        assert_eq!(c[2].what, r"drive C:\ has 4095 MB free, and a run writes there: XCL's data and logs, the GPU driver's own shader cache");
        assert!(c[2].fix.as_deref().is_some_and(|f| f.contains("DXCache") && f.contains("Shader Cache Size")), "{:?}", c[2]);
    }

    /// Folders on one drive are one drive (whatever the letter's case), in the order they come;
    /// a folder without a drive and a drive that does not answer are left out.
    #[test]
    fn the_folders_a_run_writes_are_grouped_by_drive() {
        use std::path::PathBuf;
        let folders = [
            ("data", PathBuf::from(r"c:\Users\x\AppData\Local\XCL")),
            ("cache", PathBuf::from(r"D:\cache")),
            ("driver cache", PathBuf::from(r"C:\Users\x\AppData\Local")),
            ("relative", PathBuf::from(r"logs\here")),
            ("gone", PathBuf::from(r"Q:\nothing")),
            ("share", PathBuf::from(r"\\server\share\xcl")),
        ];
        let free = |root: &str| match root {
            r"C:\" => Some(1u64 << 30),
            r"D:\" => Some(9 << 30),
            r"\\server\share\" => Some(2 << 30),
            _ => None,
        };
        assert_eq!(
            drives(&folders, free),
            [
                Drive { root: r"C:\".into(), free: 1 << 30, holds: vec!["data", "driver cache"] },
                Drive { root: r"D:\".into(), free: 9 << 30, holds: vec!["cache"] },
                Drive { root: r"\\server\share\".into(), free: 2 << 30, holds: vec!["share"] },
            ]
        );
    }

    /// The real call answers for the drive the tests run on, and not for a volume that is not there.
    #[test]
    fn the_temp_folders_drive_has_a_free_space() {
        let root = drive_root(&std::env::temp_dir()).expect("the temp folder is on a drive");
        assert!(free_bytes(&root).is_some(), "{root}");
        assert_eq!(free_bytes(r"\\?\Volume{00000000-0000-0000-0000-000000000000}\"), None);
    }

    #[test]
    fn checks_round_trip_through_the_report() {
        let c = checks(&Found { dxc: Err("x".into()), ..found(gpu()) });
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("\"level\":\"error\""), "{json}");
        assert_eq!(serde_json::from_str::<Vec<Check>>(&json).unwrap(), c);
    }
}
