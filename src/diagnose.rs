//! Read the logs a session leaves behind and say why neural rendering is or is
//! not running. Answers the commonest report ("I enabled it, nothing changed")
//! without a round trip: everything needed is already in `ReShade.log` and,
//! on the Feeder path, `dlss5-feed.log` next to the game exe.

use crate::game::{self, GameStatus};
use anyhow::Result;
use std::borrow::Cow;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Bad,
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub level: Level,
    pub text: String,
}

/// `NVSDK_NGX_*_Init -> 0xBAD00001` in a log: NGX itself refused. Add what the
/// system says about NGX Core, which is the usual cause on capable hardware.
/// `exe` is the game (and, for a 32-bit game, its helper) whose Windows GPU
/// preference is worth naming: on a hybrid machine a process started on the
/// iGPU gets exactly this error, because NGX does not exist there (#25).
fn ngx_init_failure_for(log: &str, exe: Option<&std::path::Path>, out: &mut Vec<Finding>) {
    let Some(line) = log
        .lines()
        .find(|l| l.contains("NVSDK_NGX") && l.contains("Init") && l.contains("0xBAD00001"))
    else {
        return;
    };
    if crate::gpupref::hybrid() {
        let set = exe.is_some_and(|e| {
            crate::gpupref::get(e).is_some_and(|v| crate::gpupref::is_high_performance(&v))
        });
        let names = crate::gpupref::real_adapters();
        out.push(bad(t!(
            "diagnose.ngx_init_hybrid_vendors",
            vendors = names.join(", "),
            note = if set {
                t!("diagnose.ngx_init_hybrid_pref_set")
            } else {
                Cow::Borrowed("")
            }
        )));
    }
    let system = crate::ngx::describe();
    // Reported on three machines (RTX 4070, 5080, 5090) with NGX Core present and
    // driver 616.56, always on the Feeder's own in-process D3D12 device. The same
    // chain initialises NGX fine in the 32-bit host64 helper (a separate process)
    // and on the native path where the game owns the device, so the installed
    // files are not what decides it.
    let advice = if crate::ngx::healthy() {
        t!("diagnose.ngx_init_advice_healthy")
    } else {
        t!("diagnose.ngx_init_advice_fix_first")
    };
    out.push(bad(t!(
        "diagnose.ngx_init_refused",
        line = line.trim(),
        system = system,
        advice = advice
    )));
}

/// Newest Feeder known at build time; only used to nudge users off stale copies.
const CURRENT_FEEDER: &str = "0.15.1";

fn version_key(v: &str) -> Vec<u64> {
    v.split(['.', '-'])
        .map(|p| p.parse::<u64>().unwrap_or(0))
        .collect()
}

fn ok(t: impl Into<String>) -> Finding {
    Finding {
        level: Level::Ok,
        text: t.into(),
    }
}
fn warn(t: impl Into<String>) -> Finding {
    Finding {
        level: Level::Warn,
        text: t.into(),
    }
}
fn bad(t: impl Into<String>) -> Finding {
    Finding {
        level: Level::Bad,
        text: t.into(),
    }
}

fn read(dir: &Path, name: &str) -> Option<String> {
    fs::read_to_string(dir.join(name)).ok()
}

/// The game created a DLSS session of its own (render size below output size)
/// while DLSS5-Feeder is feeding frames: two DLSS sessions in one game, which
/// crashed Kingdom Come: Deliverance II when a save loaded (#118). The Feeder's
/// own session renders at the output size unless `work_resolution` is under
/// 100%, so with that at 100 any smaller render size is the game's.
fn game_own_dlss_note(rs_log: &str, feed_cfg: Option<&str>) -> Option<String> {
    let work = feed_cfg
        .and_then(|c| {
            c.lines()
                .find_map(|l| l.trim().strip_prefix("work_resolution="))
        })
        .and_then(|v| v.trim().trim_end_matches('%').parse::<u32>().ok())
        .unwrap_or(100);
    if work != 100 {
        return None;
    }
    let re = regex::Regex::new(
        r"NGX create contract:.*feature=1 \(DLSS/DLAA\).*render=(\d+)x(\d+) out=(\d+)x(\d+)",
    )
    .unwrap();
    rs_log.lines().find_map(|l| {
        let c = re.captures(l)?;
        let n = |i: usize| c[i].parse::<u64>().ok();
        let (rw, rh, ow, oh) = (n(1)?, n(2)?, n(3)?, n(4)?);
        (rw * rh < ow * oh)
            .then(|| t!("diagnose.game_own_dlss", rw = rw, rh = rh, ow = ow, oh = oh).into_owned())
    })
}

/// The exe ReShade actually loaded into, from its first line:
/// `... loaded from '...dxgi.dll' into 'C:\\...bg3_dx11.exe' (0x...)`.
fn reshade_host_exe(log: &str) -> Option<String> {
    let line = log
        .lines()
        .find(|l| l.contains("loaded from") && l.contains(" into "))?;
    let path = line.split(" into ").nth(1)?.split('\'').nth(1)?;
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
}

/// Findings for a game folder, in reading order.
pub fn diagnose(st: &GameStatus) -> Vec<Finding> {
    let d = st.game_dir();
    let mut out = Vec::new();
    let consumer = st.consumer_dir();
    let mut rs_log = read(&consumer, "ReShade.log").or_else(|| read(&consumer, "ReShade2.log"));
    // Rockstar's Launcher.exe sits in RDR2's folder and loads dxgi.dll too, so
    // the newest ReShade.log is the launcher's 30-second session and the game's
    // own — the one with the crash — is ReShade.log1. Read that one when the
    // newest log names a different exe and the older one names ours (#97).
    let ours = st
        .exe
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase());
    let mut log_note = None;
    if let (Some(rs), Some(ours), false) = (rs_log.as_deref(), ours.as_deref(), st.uses_host()) {
        if let Some(loaded) = reshade_host_exe(rs).filter(|h| h.to_ascii_lowercase() != ours) {
            if let Some(prev) = read(&consumer, "ReShade.log1")
                .filter(|p| reshade_host_exe(p).is_some_and(|h| h.to_ascii_lowercase() == ours))
            {
                log_note = Some(ok(t!("diagnose.launcher_log_note", loaded = loaded)));
                rs_log = Some(prev);
            }
        }
    }
    out.extend(log_note);
    // Wine and Proton substitute their own d3dcompiler_47.dll, whose HLSL
    // compiler is vkd3d-shader. It does not implement every attribute ReShade
    // emits, and says so in its own words (#70).
    let wine_hlsl = rs_log
        .as_deref()
        .is_some_and(|l| l.contains("not yet implemented feature"));

    // ── a Windows compatibility layer on the exe ───────────────────
    // Watch Dogs 1 (#62) ran under apphelp/AcGenral and got SuperSampling ->
    // 0xBAD00002 on every attempt; the Feeder's author reads PlatformError
    // from a query that touches no device as a process reporting an older
    // Windows, which is exactly what a shim does (DLSS5-Feeder#47).
    if let Some(layer) = crate::library::compat_layer(&st.exe) {
        let refused = rs_log
            .as_deref()
            .into_iter()
            .chain(read(&consumer, "dlss5-feed.log").as_deref())
            .chain(read(&consumer, "dlss5-feed-host.log").as_deref())
            .any(|l| l.contains("0xBAD00001") || l.contains("0xBAD00002"));
        let text = t!("diagnose.compat_layer", layer = layer).into_owned();
        out.push(if refused { bad(text) } else { warn(text) });
    }

    // ── a game-shipped HLSL compiler shadowing the system one ──────
    // The add-on compiles its NR pass at cs_5_1. A d3dcompiler_47.dll that
    // ships with the game is loaded in preference to System32's, and an old
    // one does not know that target: "error X3506: unrecognized compiler
    // target" and no neural rendering, with everything else looking correct.
    let compiler = d.join("d3dcompiler_47.dll");
    if compiler.is_file() && !wine_hlsl {
        let ver = crate::ngx::file_version(&compiler)
            .unwrap_or_else(|| t!("diagnose.unknown_version").into_owned());
        out.push(warn(t!("diagnose.shipped_compiler", ver = ver)));
    }

    // ── dgVoodoo's reported VRAM vs what the game was told ─────────
    // A game that reads VRAM from the adapter and sizes its own pools from it
    // has to be told the same number, or it sizes for one figure and allocates
    // against another. GTA IV is the case in hand: with dgVoodoo reporting
    // 4096 MB and no matching -availablevidmem it black-screens after load,
    // and with no conf at all it takes dgVoodoo's stock 256 MB and dies with
    // "TEXP60: Unable to create color render target" (#69).
    let conf = d.join("dgVoodoo.conf");
    if conf.is_file() {
        let vram = fs::read_to_string(&conf).ok().and_then(|t| {
            t.lines()
                .filter_map(|l| l.split_once('='))
                .find(|(k, _)| k.trim().eq_ignore_ascii_case("VRAM"))
                .and_then(|(_, v)| v.trim().trim_end_matches("MB").trim().parse::<u32>().ok())
        });
        let cmdline = d.join("commandline.txt");
        if let (Some(vram), true) = (vram, cmdline.is_file()) {
            let text = fs::read_to_string(&cmdline).unwrap_or_default();
            if !text.to_ascii_lowercase().contains("-availablevidmem") {
                out.push(warn(t!(
                    "diagnose.dgvoodoo_vram",
                    vram = vram,
                    vidmem = vram.saturating_sub(64).max(256)
                )));
            }
        }
    }

    // ── which neural model is installed ────────────────────
    // Two builds of nvngx_dlssnr.dll are in circulation and only the version
    // resource separates them; every failing RTX 50 report so far carries the
    // .SF one, so the log has to name it.
    for p in [d.join(game::DLSSNR_DLL), consumer.join(game::DLSSNR_DLL)] {
        if !p.is_file() {
            continue;
        }
        if let Some(v) = crate::ngx::file_version(&p) {
            out.push(ok(t!(
                "diagnose.model_note",
                place = if p.parent() == Some(d) {
                    t!("diagnose.model_beside_exe")
                } else {
                    t!("diagnose.model_in_host64")
                },
                ver = v,
                build = crate::ngx::model_build(&v)
            )));
        }
        break;
    }

    // ── ReShade side ────────────────────────────────────────────────
    // The DLSS 5 add-on runs under the ReShade in `consumer_dir()`: beside the
    // exe for a 64-bit game, in host64\ for a 32-bit one. Reading the game
    // folder's log for a 32-bit game reads the *feeder's* 32-bit ReShade, which
    // never loads the add-on, so every 32-bit report came back "the add-on
    // never registered" no matter how healthy the install was (#69).
    // A 32-bit game has two ReShades: the one beside the exe, which the Home key
    // opens and which loads the feeder, and the 64-bit one in host64\ that hosts
    // the neural add-on. Reporting only the second leaves "Home does nothing"
    // unexplained, which is the first thing the player actually notices (#69).
    if st.uses_host() && !game::is_reshade_dll(&d.join(game::RESHADE_PROXY)) {
        out.push(bad(t!(
            "diagnose.no_reshade_beside_exe",
            proxy = game::RESHADE_PROXY
        )));
    }
    let Some(rs) = rs_log else {
        out.push(bad(if st.uses_host() {
            t!("diagnose.no_host64_reshade_log")
        } else {
            t!("diagnose.no_reshade_log")
        }));
        return out;
    };
    if rs.contains("Initializing crosire's ReShade") {
        out.push(ok(t!("diagnose.reshade_loaded")));
    }
    // Neural Upstream stands in for the RenoDX add-on and logs under its own
    // tag. Reading its log for the RenoDX add-on's lines reported "the add-on
    // never registered" and "no NGX call was intercepted" on an install whose
    // log showed the add-on registered and hooked (#86).
    if st.upstream {
        out.extend(upstream_findings(&rs));
        return out;
    }
    // ShortFuse's add-on logs as "RenoDX DLSS", not as the DLSS 5 add-on, so
    // the DLSS 5 add-on's lines would all read as missing.
    if st.sf && !st.dlss5_addon {
        out.extend(sf_findings(&rs));
        return out;
    }
    let failed_line = rs
        .lines()
        .find(|l| l.contains("Failed to load add-on") && l.contains("renodx-dlss5"));
    if let Some(l) = failed_line {
        let code = l
            .rsplit("error code ")
            .next()
            .unwrap_or("")
            .trim_end_matches('!');
        let extra = match code.trim() {
            "2148073478" => t!("diagnose.addon_refused_unsigned"),
            "1114" => t!("diagnose.addon_refused_entry_point"),
            _ => Cow::Borrowed(""),
        };
        out.push(bad(t!(
            "diagnose.addon_refused",
            code = code,
            extra = extra
        )));
    } else if rs.contains("DLSS 5 Neural Rendering") {
        out.push(ok(t!("diagnose.addon_registered")));
    } else {
        out.push(bad(t!(
            "diagnose.addon_never_registered",
            place = if st.uses_host() {
                t!("diagnose.addon_never_registered_host64")
            } else {
                t!("diagnose.addon_never_registered_game_folder")
            }
        )));
    }
    if rs.contains("NR toggled ON") && !rs.contains("NR toggled OFF") {
        out.push(ok(t!("diagnose.nr_toggled_on")));
    } else if rs.contains("NR toggled OFF") {
        out.push(warn(t!("diagnose.nr_toggled_off_maybe")));
    }
    if rs.contains("inline feature 18 evaluation succeeded") {
        // "Ran" is not "ran on your frames". Cyberpunk 2077 drove the feature
        // through 18 re-creations and four worksets in four minutes, reaching
        // sixty evaluations in 95 seconds — under one frame a second — and
        // Diagnose called that success (#95). Count the churn.
        let created = rs.matches("feature 18 created via").count();
        let worksets = rs.matches("second NR workset is live").count();
        // Every F6 and every slider drag re-creates the feature too, so a
        // count alone is not churn: a Prey session with 93 creations had
        // reached 600 straight evaluations first (#99). The add-on logs the
        // count at 1, 60 and 600; reaching 600 means the pass held.
        let held = rs.contains("evaluation succeeded (count=600");
        if worksets >= 2 || (created >= 6 && !held) {
            out.push(bad(t!(
                "diagnose.nr_churn",
                created = created,
                worksets = worksets + 1
            )));
        } else {
            out.push(ok(t!("diagnose.nr_ran")));
        }
    } else if rs.contains("feature=1 (DLSS/DLAA)") {
        out.push(warn(t!("diagnose.dlss_seen_not_evaluated")));
    } else if st.mode == game::Mode::Native {
        if st.feeder {
            out.push(warn(t!("diagnose.native_feeder_leftover")));
        }
        // The add-on hooks NVSDK_NGX_D3D12_*. A game whose DLSS runs on D3D11
        // calls the D3D11 entry points, which it never sees, so "no create"
        // is expected until the bridge is installed (#33, BG3 DX11).
        if st.needs_bridge() && !st.bridge {
            out.push(bad(t!("diagnose.no_ngx_call_needs_bridge")));
        } else {
            out.push(bad(t!("diagnose.no_ngx_call_intercepted")));
        }
    }

    // Linux/Proton: ReShade generates HLSL with [fastopt] for shader model 4
    // and up, and Wine's d3dcompiler_47 (vkd3d-shader) has not implemented it,
    // so DLSS5_Feed.fx and the Lumenite shaders never build — with the feed
    // add-on then reporting its technique missing, which reads like our bug
    // rather than a missing compiler (#70).
    if wine_hlsl {
        let line = rs
            .lines()
            .find(|l| l.contains("not yet implemented feature"))
            .unwrap_or("")
            .trim();
        out.push(bad(t!("diagnose.wine_hlsl", line = line)));
    }

    // The compile failure itself, which is unambiguous when it appears.
    if let Some(line) = rs
        .lines()
        .find(|l| l.contains("X3506") || l.contains("unrecognized compiler target"))
    {
        out.push(bad(t!("diagnose.x3506", line = line.trim())));
    }

    // A game with more than one executable (a Vulkan build and a DX11 build,
    // a launcher and the game) can be installed for one and played through
    // another: ReShade loads, everything looks right, nothing is hooked (#33).
    if let Some(loaded) = reshade_host_exe(&rs).filter(|_| !st.uses_host()) {
        let ours = st
            .exe
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_lowercase());
        if ours.is_some_and(|o| o != loaded.to_ascii_lowercase()) {
            out.push(warn(t!(
                "diagnose.wrong_exe",
                loaded = loaded,
                ours = st.exe.file_name().unwrap_or_default().to_string_lossy()
            )));
        }
    }

    // ── DX11 bridge (native DLSS on D3D11) ──────────────────────────
    if let Some(bl) = read(d, "dlss5-bridge.log") {
        if let Some(line) = bl.lines().rev().find(|l| l.contains("stopped:")) {
            out.push(bad(t!("diagnose.bridge_stopped", line = line.trim())));
        }
        if let Some(line) = bl
            .lines()
            .find(|l| l.contains("D3D12CreateDevice failed 0x887E0003"))
        {
            // Where the redist actually is decides what the user can do. Unreal
            // puts it in a D3D12 subfolder; a Unity player declares the exe's own
            // folder, so renaming a "D3D12 folder" that was never there changes
            // nothing and reads as a dead end (dlss5-bridge#24).
            let where_ = match game::has_agility_redist(d) {
                Some(p) => {
                    let ver = crate::ngx::file_version(&p)
                        .unwrap_or_else(|| t!("diagnose.unknown_version").into_owned());
                    t!(
                        "diagnose.agility_redist_here",
                        path = p.display(),
                        ver = ver
                    )
                    .into_owned()
                }
                None => t!("diagnose.agility_redist_missing").into_owned(),
            };
            out.push(bad(t!(
                "diagnose.agility_invalid_redist",
                line = line.trim(),
                at = where_
            )));
        } else if bl.contains("frames:") && !bl.contains("session failed") {
            out.push(ok(t!("diagnose.bridge_ok")));
        }
    }

    // ── Feeder side (games without DLSS) ────────────────────────────
    if st.mode == game::Mode::Feeder {
        let Some(fd) = read(d, "dlss5-feed.log") else {
            out.push(bad(t!("diagnose.no_feed_log")));
            return out;
        };
        if fd.contains("feature ready") {
            out.push(ok(t!("diagnose.feeder_feature_ready")));
        }
        if fd.contains("frame") && fd.contains("delivered") {
            out.push(ok(t!("diagnose.frames_delivered")));
        }
        if fd.contains("technique MISSING") && !fd.contains("technique found") {
            out.push(bad(t!("diagnose.feed_technique_missing")));
        }
        // The first effects line of a session always says "none": effects are
        // not compiled yet. Only the last one describes the running state (#6).
        let last_effects = fd
            .lines()
            .rev()
            .find(|l| l.contains("[feed] effects:"))
            .unwrap_or("");
        if last_effects.contains("-> none (not installed)") {
            out.push(bad(t!("diagnose.mv_provider_not_enabled")));
        }
        ngx_init_failure_for(&fd, Some(&st.exe), &mut out);
        if let Some(note) = game_own_dlss_note(&rs, read(d, "dlss5-feed.cfg").as_deref()) {
            out.push(warn(note));
        }
        // The normal 64-bit add-on stops itself on a Direct3D 9 or 10 game.
        if fd.contains("only Direct3D 11/12, Vulkan and OpenGL games are supported") {
            out.push(bad(if st.helper {
                t!("diagnose.helper_api_unsupported")
            } else {
                t!("diagnose.normal_api_unsupported")
            }));
        }
        // 32-bit games: the work happens in host64\, and its own log names the reason.
        if let Some(hl) = read(&d.join(game::HOST_DIR), "dlss5-feed-host.log") {
            if hl.contains("feature ready") {
                out.push(ok(t!("diagnose.host_feature_ready")));
            }
            ngx_init_failure_for(&hl, Some(&st.consumer_dir().join(game::HOST_EXE)), &mut out);
        } else if st.uses_host() {
            out.push(warn(t!("diagnose.no_host_log_yet")));
        }
        if let Some(ver) = fd
            .lines()
            .next()
            .and_then(|l| {
                // "HH:MM:SS.mmm  dlss5-feed 0.12.0 (built ...) attached."
                let mut it = l.split_whitespace();
                it.find(|t| t.starts_with("dlss5-feed"))?;
                it.next()
            })
            .filter(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit()))
        {
            if version_key(ver) < version_key(CURRENT_FEEDER) {
                out.push(warn(t!(
                    "diagnose.stale_feeder",
                    ver = ver,
                    current = CURRENT_FEEDER
                )));
            }
        }
        // The last probe is the verdict: the first ones run on a menu or a
        // loading screen. And "100% non-zero" contains "0% non-zero", so the
        // substring test reported every healthy log as all-zero (#99).
        if let Some(l) = fd.lines().rev().find(|l| l.contains("MV probe")) {
            let pct = l
                .split("non-zero")
                .next()
                .and_then(|p| p.rsplit(',').next())
                .and_then(|p| p.trim().trim_end_matches('%').parse::<u32>().ok());
            if pct == Some(0) {
                out.push(bad(t!("diagnose.mv_all_zero")));
            }
        }
        if fd
            .lines()
            .rev()
            .find(|l| l.contains("Depth probe"))
            .is_some_and(|l| l.contains("flat"))
        {
            out.push(bad(t!("diagnose.depth_flat")));
        }
        if fd.contains("DLSS super sampling is not available") {
            out.push(bad(t!("diagnose.dlss_unavailable")));
        }
        // Some games refuse the reduced work-resolution path: the feed builds
        // its shared textures, the staging SRV for the smaller image fails, and
        // three failed builds stop the feed. Nothing downstream then happens —
        // the add-on's overlay says "HOOKS ARMED - NO DLSS CREATE SEEN" and
        // toggling neural rendering in game does nothing, which reads like the
        // add-on is broken rather than one setting being wrong (#74).
        // The 32-bit halves talk a versioned protocol. When they disagree the
        // host exits immediately and the game shows nothing at all (#69).
        if let Some(line) = fd
            .lines()
            .chain(
                read(&st.consumer_dir(), "dlss5-feed-host.log")
                    .as_deref()
                    .unwrap_or("")
                    .lines(),
            )
            .find(|l| l.contains("speaks protocol v") && l.contains("this host v"))
        {
            out.push(bad(t!("diagnose.protocol_mismatch", line = line.trim())));
        }
        // The host warns when the add-on's version and the driver are a pair it
        // has measured failing. Every build of that add-on carries the same
        // embedded FileVersion (0.2026.0828.0517 in 4.55 and 4.70 alike), so
        // the warning fires on the classic build too — the one the host's own
        // text calls a way through. Only the tag this tool recorded can say
        // which build is actually on disk (#69, and upstream DLSS5-Feeder#90).
        if fd.contains("is a combination measured to fail")
            || read(&st.consumer_dir(), "dlss5-feed-host.log")
                .as_deref()
                .is_some_and(|l| l.contains("is a combination measured to fail"))
        {
            let tag = read(&st.consumer_dir(), crate::game::DLSS5_ADDON_MARKER)
                .map(|t| t.trim().to_owned())
                .unwrap_or_default();
            if tag == crate::installer::RENODX_CLASSIC_TAG {
                out.push(ok(t!("diagnose.measured_fail_already_classic", tag = tag)));
            } else {
                out.push(warn(t!(
                    "diagnose.measured_fail_pin_classic",
                    tag = crate::installer::RENODX_CLASSIC_TAG
                )));
            }
        }
        // The create faults inside the driver rather than returning a code. The
        // feed catches it, cannot retry (the consumer's own locks were skipped
        // by the unwind), and stops — so the game runs and nothing happens,
        // with the reason six lines up in a log nobody reads (#76).
        if fd.contains("CreateFeature raised 0xC0000005") {
            let stack = fd
                .lines()
                .find(|l| l.contains("CreateFeature fault stack"))
                .map(|l| l.split("(innermost first):").nth(1).unwrap_or(l).trim())
                .unwrap_or("")
                .to_owned();
            let two_modules = fd.contains("two copies of the DLSS NGX module are loaded");
            let mut t = t!("diagnose.feature_create_fault").into_owned();
            if !stack.is_empty() {
                t.push_str(&t!("diagnose.fault_stack", stack = stack));
            }
            if two_modules {
                // Tested on Proton and it is the wrong advice there: with the
                // game-local DLL moved aside, the driver's own NGX answers
                // 0xBAD00012 (NotImplemented) for SuperSampling and DLSS is not
                // available at all, which is worse than the crash (#76).
                if wine_hlsl || fd.contains("driver 999.99") {
                    t.push_str(&t!("diagnose.nvngx_advice_proton"));
                } else {
                    t.push_str(&t!("diagnose.nvngx_advice_windows"));
                }
            }
            out.push(bad(t));
        }
        if fd.contains("work-resolution staging SRV failed") {
            let pct = fd
                .lines()
                .find(|l| l.contains("work resolution ("))
                .and_then(|l| l.split("work resolution (").nth(1))
                .and_then(|r| r.split(')').next())
                .map(|p| p.to_owned())
                .unwrap_or_else(|| t!("diagnose.work_res_below_100").into_owned());
            out.push(bad(t!("diagnose.work_res_srv_failed", pct = pct)));
        }
        if fd.contains("stopped:") {
            let line = fd
                .lines()
                .rev()
                .find(|l| l.contains("stopped:"))
                .unwrap_or("")
                .trim()
                .to_string();
            out.push(bad(t!("diagnose.feed_stopped", line = line)));
        }
        if fd.contains("CRASH RECORDED") {
            out.push(warn(t!("diagnose.crash_recorded")));
        }
    }
    out
}

/// Read the game folder and produce findings, or a single fatal one.
pub fn run(exe: &Path) -> Result<Vec<Finding>> {
    let st = game::inspect(exe)?;
    Ok(diagnose(&st))
}

/// ShortFuse's add-on (`renodx-dlss.addon64`), read by its own log lines.
fn sf_findings(rs: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    if let Some(l) = rs
        .lines()
        .find(|l| l.contains("Failed to load add-on") && l.contains("renodx-dlss.addon64"))
    {
        let unknown = t!("diagnose.unknown_version");
        let code = l
            .split("error code ")
            .nth(1)
            .map(|c| c.trim().trim_end_matches('!'))
            .unwrap_or(unknown.as_ref());
        out.push(bad(t!("diagnose.sf_refused", code = code)));
        return out;
    }
    if rs.contains("failed to register RenoDX DLSS") {
        out.push(bad(t!("diagnose.sf_not_registered")));
        return out;
    }
    if rs.contains("RenoDX DLSS attached") {
        out.push(ok(t!("diagnose.sf_attached")));
    } else {
        out.push(bad(t!("diagnose.sf_never_attached")));
        return out;
    }
    if rs.contains("could not attach the direct nvngx_dlssnr.dll runtime")
        || rs.contains("nvngx_dlssnr.dll missing")
    {
        out.push(bad(t!("diagnose.sf_model_missing")));
    }
    if rs.contains("NR device feature initialization failed") {
        out.push(bad(t!("diagnose.sf_device_init_failed")));
    }
    let failed = rs.matches("NR evaluation failed").count();
    let ran: u64 = rs
        .lines()
        // "detached signed snippet after N successful evaluations"
        .filter_map(|l| {
            let rest = l.split("snippet after ").nth(1)?;
            let (n, _) = rest.split_once(" successful evaluations")?;
            n.trim().parse().ok()
        })
        .max()
        .unwrap_or(0);
    if ran > 0 {
        out.push(ok(t!("diagnose.sf_ran", ran = ran)));
    } else if failed > 0 {
        out.push(bad(t!("diagnose.sf_failed_count", failed = failed)));
    } else {
        out.push(warn(t!("diagnose.sf_no_eval")));
    }
    out
}

/// What matiasLombo's neural-upstream (`nvngx.dll.addon64`, "DLSS5 NR
/// Pre-Upscale") wrote to ReShade.log, read against its own source: the game's
/// DLSS is logged as `CreateFeature id=1` (a DLSS create carries no DLSSNR
/// slots, so `<no slot>` there is normal), every DLSS evaluate as `eval feat=`,
/// and the add-on's own network as `2b: SNIPPET CreateFeature(18, …)` followed
/// by `2c: NR Evaluate(…) -> 0x00000001` once it ran.
fn upstream_findings(rs: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    if let Some(l) = rs
        .lines()
        .find(|l| l.contains("Failed to load add-on") && l.contains("nvngx.dll.addon64"))
    {
        let code = l
            .rsplit("error code ")
            .next()
            .unwrap_or("")
            .trim_end_matches('!');
        out.push(bad(t!("diagnose.upstream_refused", code = code)));
        return out;
    }
    if rs.contains("[NRPRE] addon registered") || rs.contains("DLSS5 NR Pre-Upscale") {
        out.push(ok(t!("diagnose.upstream_registered")));
    } else {
        out.push(bad(t!("diagnose.upstream_never_registered")));
        return out;
    }
    if rs.contains("[NRPRE] no NGX evaluate could be hooked") {
        out.push(bad(t!("diagnose.upstream_no_evaluate_hook")));
        return out;
    }
    let nr_ran = rs
        .lines()
        .any(|l| l.contains("2c: NR Evaluate(") && l.contains("-> 0x00000001"));
    let nr_failed = rs
        .lines()
        .find(|l| l.contains("NR Evaluate(") && l.contains("FAILED"));
    let snippet_failed = rs
        .lines()
        .find(|l| l.contains("SNIPPET CreateFeature(18") && !l.contains("-> 0x00000001"));
    if nr_ran {
        out.push(ok(t!("diagnose.upstream_ran")));
    } else if let Some(l) = nr_failed {
        out.push(bad(t!("diagnose.upstream_eval_failed", line = l.trim())));
    } else if let Some(l) = snippet_failed {
        out.push(bad(t!("diagnose.upstream_snippet_failed", line = l.trim())));
    } else if rs.contains("2b: core exports missing") {
        out.push(bad(t!("diagnose.upstream_exports_missing")));
    } else if rs.contains("[NRPRE] eval feat=") {
        out.push(warn(t!("diagnose.upstream_no_network")));
    } else if rs.contains("[NRPRE] CreateFeature id=") {
        out.push(warn(t!("diagnose.upstream_never_rendered")));
    } else {
        out.push(bad(t!("diagnose.no_ngx_call_intercepted")));
    }
    out
}

#[cfg(test)]
mod tests {
    /// Baldur's Gate 3 ships bg3.exe (Vulkan) and bg3_dx11.exe; installing for
    /// one and playing the other leaves everything looking right and nothing
    /// hooked (#33). The exe name is in ReShade's first line.
    #[test]
    fn reshade_host_exe_reads_the_first_line() {
        let log = "01:30:11:810 [17792] | INFO  | Initializing crosire's ReShade version '6.8.0.2155' (64-bit) loaded from 'C:\\\\Program Files (x86)\\\\Steam\\\\steamapps\\\\common\\\\Baldurs Gate 3\\\\bin\\\\dxgi.dll' into 'C:\\\\Program Files (x86)\\\\Steam\\\\steamapps\\\\common\\\\Baldurs Gate 3\\\\bin\\\\bg3_dx11.exe' (0x64317982) ...";
        assert_eq!(
            super::reshade_host_exe(log).as_deref(),
            Some("bg3_dx11.exe")
        );
        assert!(super::reshade_host_exe("nothing useful here").is_none());
    }

    use super::*;
    use crate::game::testutil::*;

    fn setup(feeder: bool) -> (tempfile::TempDir, std::path::PathBuf) {
        std::env::set_var("DLSS5ONECLICK_SKIP_GPU_CHECK", "1");
        let t = tempfile::tempdir().unwrap();
        let exe = make_pe(&t.path().join("game.exe"), game::PE_X64);
        if !feeder {
            fs::write(t.path().join(game::DLSS_DLL), b"x").unwrap();
        }
        (t, exe)
    }

    /// A 32-bit game runs the add-on under the 64-bit ReShade in host64\, so
    /// that is the log to read. Reading the game folder's log — the feeder's
    /// own 32-bit ReShade, which never loads the add-on — reported "the add-on
    /// never registered" on installs that were fine (#69).
    #[test]
    fn thirty_two_bit_reads_the_host64_reshade_log() {
        std::env::set_var("DLSS5ONECLICK_SKIP_GPU_CHECK", "1");
        let t = tempfile::tempdir().unwrap();
        let exe = make_pe(&t.path().join("game.exe"), game::PE_X86);
        let host = t.path().join(game::HOST_DIR);
        fs::create_dir_all(&host).unwrap();
        // The 32-bit ReShade beside the exe: no add-on, and never will have one.
        fs::write(
            t.path().join("ReShade.log"),
            "Initializing crosire's ReShade version '6.8.0'\n",
        )
        .unwrap();
        // The one that matters, in host64\.
        fs::write(
            host.join("ReShade.log"),
            "Initializing crosire's ReShade version '6.8.0'\nRegistered add-on \"DLSS 5 Neural Rendering\"\n",
        )
        .unwrap();
        let f = run(&exe).unwrap();
        assert!(
            f.iter()
                .any(|x| x.level == Level::Ok && x.text.contains("add-on registered")),
            "{f:?}"
        );
        assert!(
            !f.iter().any(|x| x.text.contains("never registered")),
            "{f:?}"
        );
    }

    /// And when host64\ has no ReShade log at all, say so in host64 terms
    /// rather than claiming ReShade never loaded beside the exe.
    #[test]
    fn thirty_two_bit_missing_host64_log_names_host64() {
        std::env::set_var("DLSS5ONECLICK_SKIP_GPU_CHECK", "1");
        let t = tempfile::tempdir().unwrap();
        let exe = make_pe(&t.path().join("game.exe"), game::PE_X86);
        fs::write(
            t.path().join("ReShade.log"),
            "Initializing crosire's ReShade version '6.8.0'\n",
        )
        .unwrap();
        let f = run(&exe).unwrap();
        assert!(f.iter().any(|x| x.text.contains("host64")), "{f:?}");
    }

    /// Under Proton the effects are compiled by Wine's d3dcompiler_47
    /// (vkd3d-shader), which has not implemented [fastopt]. Saying "the add-on
    /// never registered" or "rename your d3dcompiler" sends the user in the
    /// wrong direction — the second one actively removes the working compiler (#70).
    #[test]
    fn wine_hlsl_compiler_is_named_and_rename_advice_suppressed() {
        let (t, exe) = setup(true);
        fs::write(t.path().join("d3dcompiler_47.dll"), b"MZ").unwrap();
        fs::write(
            t.path().join("ReShade.log"),
            "Initializing crosire's ReShade version '6.8.0'\n\
             ERROR | Failed to compile 'DLSS5_Feed.fx':\n\
             <anonymous>:118:13: E5017: Aborting due to not yet implemented feature: Unhandled attribute 'fastopt'.\n",
        )
        .unwrap();
        let f = run(&exe).unwrap();
        assert!(
            f.iter()
                .any(|x| x.level == Level::Bad && x.text.contains("vkd3d-shader")),
            "{f:?}"
        );
        assert!(
            !f.iter().any(|x| x.text.contains("d3dcompiler_47.dll.bak")),
            "{f:?}"
        );
        // The recipe a reporter measured working, not just "install a compiler" (#76).
        assert!(
            f.iter().any(|x| x.text.contains("WINEDLLOVERRIDES")),
            "{f:?}"
        );
    }

    /// Dying Light refuses the reduced work-resolution path. The user sees a
    /// stopped feed and an add-on saying it never saw a DLSS create, with
    /// nothing naming the one setting responsible (#74).
    #[test]
    fn work_resolution_build_failure_names_the_setting() {
        let (t, exe) = setup(true);
        fs::write(
            t.path().join("ReShade.log"),
            "Initializing crosire's ReShade\nRegistered add-on \"DLSS 5 Neural Rendering\"\n",
        )
        .unwrap();
        fs::write(
            t.path().join("dlss5-feed.log"),
            "[feed] building: 2176x1224 work resolution (85%) -> 2560x1440 backbuffer\n\
             [feed] work-resolution staging SRV failed\n\
             [feed] failure: resource build\n\
             stopped: repeated failures. The game renders normally.\n",
        )
        .unwrap();
        let f = run(&exe).unwrap();
        let hit = f
            .iter()
            .find(|x| x.text.contains("work-resolution staging SRV failed"))
            .unwrap_or_else(|| panic!("{f:?}"));
        assert_eq!(hit.level, Level::Bad);
        assert!(hit.text.contains("85%"), "{}", hit.text);
        assert!(hit.text.contains("work_resolution = 100"), "{}", hit.text);
    }

    /// "Home does nothing" on a 32-bit game means the ReShade beside the exe is
    /// missing, which is upstream of every host64 finding. Diagnose used to
    /// report only the host64 side and leave the player looking in the wrong
    /// folder (#69).
    #[test]
    fn thirty_two_bit_missing_game_side_reshade_is_named_first() {
        std::env::set_var("DLSS5ONECLICK_SKIP_GPU_CHECK", "1");
        let t = tempfile::tempdir().unwrap();
        let exe = make_pe(&t.path().join("game.exe"), game::PE_X86);
        let host = t.path().join(game::HOST_DIR);
        fs::create_dir_all(&host).unwrap();
        fs::write(
            host.join("ReShade.log"),
            "Initializing crosire's ReShade version '6.8.0'\n",
        )
        .unwrap();
        let f = run(&exe).unwrap();
        assert!(
            f.iter()
                .any(|x| x.level == Level::Bad && x.text.contains("No ReShade beside the game exe")),
            "{f:?}"
        );
    }

    /// A create that faults inside the driver stops the feed for good, and the
    /// log's own hint about two NGX modules is the actionable part. Neither
    /// reached the user (#76).
    #[test]
    fn a_faulting_feature_create_is_explained_with_its_stack() {
        let (t, exe) = setup(true);
        fs::write(
            t.path().join("ReShade.log"),
            "Initializing crosire's ReShade\nRegistered add-on \"DLSS 5 Neural Rendering\"\n",
        )
        .unwrap();
        fs::write(
            t.path().join("dlss5-feed.log"),
            "[feed] CreateFeature raised 0xC0000005 (reading address 00000000575284C0) (caught; nothing submitted)\n\
             [feed] CreateFeature fault stack, by module (innermost first): d3d12core.dll <- dxgi.dll <- nvapi64.dll <- nvngx_dlss.dll <- renodx-dlss5.addon64\n\
             [feed] two copies of the DLSS NGX module are loaded (the game-local nvngx_dlss.dll and the driver's _nvngx.dll)\n",
        )
        .unwrap();
        let f = run(&exe).unwrap();
        let hit = f
            .iter()
            .find(|x| x.text.contains("faulted inside the driver"))
            .unwrap_or_else(|| panic!("{f:?}"));
        assert_eq!(hit.level, Level::Bad);
        assert!(hit.text.contains("d3d12core.dll"), "{}", hit.text);
        assert!(hit.text.contains("nvngx_dlss.dll.off"), "{}", hit.text);
    }

    /// GTA IV reads the adapter's VRAM and sizes its pools from it, so dgVoodoo
    /// reporting 4096 MB without a matching -availablevidmem is a black screen
    /// after load, and no conf at all is TEXP60 at startup (#69).
    #[test]
    fn dgvoodoo_vram_without_availablevidmem_is_flagged() {
        let (t, exe) = setup(true);
        fs::write(
            t.path().join("ReShade.log"),
            "Initializing crosire's ReShade\n",
        )
        .unwrap();
        fs::write(t.path().join("dgVoodoo.conf"), "[DirectX]\nVRAM = 4096\n").unwrap();
        fs::write(t.path().join("commandline.txt"), "-norestrictions\n").unwrap();
        let f = run(&exe).unwrap();
        let hit = f
            .iter()
            .find(|x| x.text.contains("-availablevidmem"))
            .unwrap_or_else(|| panic!("{f:?}"));
        assert_eq!(hit.level, Level::Warn);
        assert!(hit.text.contains("4096"), "{}", hit.text);

        // Already set: nothing to say.
        fs::write(
            t.path().join("commandline.txt"),
            "-availablevidmem 4032\n-norestrictions\n",
        )
        .unwrap();
        let f = run(&exe).unwrap();
        assert!(
            !f.iter().any(|x| x.text.contains("-availablevidmem")),
            "{f:?}"
        );
    }

    /// Moving the game-local nvngx_dlss.dll aside is the right advice on
    /// Windows and the wrong advice under Proton, where the driver's own NGX
    /// answers NotImplemented and DLSS disappears entirely. 0batsy tested both
    /// and neither helped, but the second left him worse off (#76).
    #[test]
    fn the_nvngx_advice_is_withheld_under_proton() {
        let (t, exe) = setup(true);
        let crash = "[feed] CreateFeature raised 0xC0000005 (caught; nothing submitted)\n\
                     [feed] two copies of the DLSS NGX module are loaded (the game-local nvngx_dlss.dll and the driver's _nvngx.dll)\n";
        fs::write(
            t.path().join("ReShade.log"),
            "Initializing crosire's ReShade\nRegistered add-on \"DLSS 5 Neural Rendering\"\n",
        )
        .unwrap();

        // Windows: the advice stands.
        fs::write(t.path().join("dlss5-feed.log"), crash).unwrap();
        let f = run(&exe).unwrap();
        assert!(
            f.iter().any(|x| x.text.contains("nvngx_dlss.dll.off")),
            "{f:?}"
        );

        // Proton, identified by the adapter line vkd3d reports.
        fs::write(
            t.path().join("dlss5-feed.log"),
            format!("[feed] adapter: NVIDIA GeForce RTX 4070 SUPER driver 999.99\n{crash}"),
        )
        .unwrap();
        let f = run(&exe).unwrap();
        assert!(
            !f.iter().any(|x| x.text.contains("nvngx_dlss.dll.off")),
            "{f:?}"
        );
        assert!(
            f.iter().any(|x| x.text.contains("Do NOT do that here")),
            "{f:?}"
        );
    }

    /// The host's warning keys on a FileVersion every build of that add-on
    /// shares, so it fires on the classic build the host itself recommends.
    /// Only the tag this tool recorded can tell them apart (#69).
    #[test]
    fn the_measured_to_fail_warning_reads_the_recorded_tag() {
        let (t, exe) = setup(true);
        fs::write(
            t.path().join("ReShade.log"),
            "Initializing crosire's ReShade\nRegistered add-on \"DLSS 5 Neural Rendering\"\n",
        )
        .unwrap();
        fs::write(
            t.path().join("dlss5-feed.log"),
            "[feed] WARNING: renodx-dlss5 v4.6 with NVIDIA driver 616.64 is a combination \
             measured to fail\n",
        )
        .unwrap();

        // No tag on disk: the advice is to re-run Install, which pins it.
        let f = run(&exe).unwrap();
        assert!(
            f.iter()
                .any(|x| x.level == Level::Warn && x.text.contains("pins the classic build")),
            "{f:?}"
        );

        // Already pinned: say so instead of sending them round again.
        fs::write(
            t.path().join(crate::game::DLSS5_ADDON_MARKER),
            crate::installer::RENODX_CLASSIC_TAG,
        )
        .unwrap();
        let f = run(&exe).unwrap();
        assert!(
            f.iter()
                .any(|x| x.level == Level::Ok && x.text.contains("cannot tell them apart")),
            "{f:?}"
        );
    }

    #[test]
    fn no_reshade_log_is_fatal() {
        let (_t, exe) = setup(true);
        let f = run(&exe).unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].level, Level::Bad);
        assert!(f[0].text.contains("never loaded"));
    }

    #[test]
    fn native_without_game_dlss_call() {
        let (t, exe) = setup(false);
        fs::write(
            t.path().join("ReShade.log"),
            "Initializing crosire's ReShade version '6.8.0'\nRegistered add-on \"DLSS 5 Neural Rendering\"\n",
        )
        .unwrap();
        let f = run(&exe).unwrap();
        assert!(f
            .iter()
            .any(|x| x.text.contains("game's own DLSS never ran")));
        assert!(f
            .iter()
            .any(|x| x.level == Level::Ok && x.text.contains("add-on registered")));
    }

    #[test]
    fn addon_load_failure_is_explained() {
        let (t, exe) = setup(false);
        fs::write(
            t.path().join("ReShade.log"),
            "Initializing crosire's ReShade\nFailed to load add-on from 'C:\\g\\renodx-dlss5.addon64' with error code 2148073478!\n",
        )
        .unwrap();
        let f = run(&exe).unwrap();
        assert!(f.iter().any(|x| x.text.contains("unsigned DLLs")));
    }

    #[test]
    fn feeder_provider_not_enabled() {
        let (t, exe) = setup(true);
        fs::write(
            t.path().join("ReShade.log"),
            "Initializing crosire's ReShade\nRegistered add-on \"DLSS 5 Neural Rendering\"\n",
        )
        .unwrap();
        fs::write(
            t.path().join("dlss5-feed.log"),
            "[feed] effects: DLSS5_Feed.fx technique found, DLSS5_MV_PROVIDER=3 (LumeniteFX Kernel) -> none (not installed)\n",
        )
        .unwrap();
        let f = run(&exe).unwrap();
        assert!(f
            .iter()
            .any(|x| x.text.contains("motion-vector provider is not enabled")));
    }

    #[test]
    fn feeder_last_effects_line_wins_and_ngx_init_failure_named() {
        let (t, exe) = setup(true);
        fs::write(
            t.path().join("ReShade.log"),
            "Initializing crosire's ReShade\nRegistered add-on \"DLSS 5 Neural Rendering\"\n",
        )
        .unwrap();
        fs::write(
            t.path().join("dlss5-feed.log"),
            "12:33:33.151  dlss5-feed 0.7.0 (built Aug 31 2026) attached.\n\
             [feed] effects: DLSS5_Feed.fx technique MISSING, DLSS5_MV_PROVIDER=3 (LumeniteFX Kernel) -> none (not installed)\n\
             [feed] effects: DLSS5_Feed.fx technique found, DLSS5_MV_PROVIDER=3 (LumeniteFX Kernel) -> Lumenite_Kernel (enabled)\n\
             [feed] NVSDK_NGX_D3D12_Init -> 0xBAD00001 (FeatureNotSupported)\n\
             stopped: the D3D12/NGX session failed to start.\n",
        )
        .unwrap();
        let f = run(&exe).unwrap();
        assert!(!f
            .iter()
            .any(|x| x.text.contains("motion-vector provider is not enabled")));
        assert!(f
            .iter()
            .any(|x| x.text.contains("NGX refused to initialise")));
        assert!(f
            .iter()
            .any(|x| x.text.contains("0.7.0 in the log is older")));
    }

    #[test]
    fn healthy_session_says_so() {
        let (t, exe) = setup(true);
        fs::write(
            t.path().join("ReShade.log"),
            "Initializing crosire's ReShade\nRegistered add-on \"DLSS 5 Neural Rendering\"\ninline feature 18 evaluation succeeded (count=60)\n",
        )
        .unwrap();
        fs::write(
            t.path().join("dlss5-feed.log"),
            "[feed] feature ready: 3840x2160 DLAA\n[feed] frame 1 delivered\n",
        )
        .unwrap();
        let f = run(&exe).unwrap();
        assert!(f.iter().all(|x| x.level == Level::Ok), "{f:?}");
        assert!(f.iter().any(|x| x.text.contains("raise NR Intensity")));
    }

    /// The lines felipeptxa's RDR2 log carried (#86): the add-on registered,
    /// hooked, and saw the game create its DLSS feature, then the session
    /// ended at the menu. Read as the RenoDX add-on's log this was two FAILs.
    #[test]
    fn neural_upstream_log_is_read_by_its_own_lines() {
        let menu = "Registered add-on \"DLSS5 NR Pre-Upscale\"
            [NRPRE] addon registered (NR at render resolution -- configure in the ReShade overlay)
            [NRPRE] hook 0 on NVSDK_NGX_D3D12_EvaluateFeature: OK
            [NRPRE] hook on NVSDK_NGX_D3D12_CreateFeature: OK (module 0)
            [NRPRE] CreateFeature id=1 -> res=0x00000001 handle=1 | NR W=4294967295 H=4294967295 ratio=-1.000 preset=-1 | DLSSNR.Color=<no slot> | DLSSNR.Output=<no slot> | DLSSNR.Depth=<no slot> | DLSSNR.MVec=<no slot>
            [NRPRE] F11: cadence phase -> 0/1
";
        let f = upstream_findings(menu);
        assert!(
            f.iter()
                .any(|x| x.level == Level::Ok && x.text.contains("registered")),
            "{f:?}"
        );
        assert!(
            f.iter().any(|x| x.text.contains("never rendered a frame")),
            "{f:?}"
        );
        assert!(
            !f.iter().any(|x| x.text.contains("never registered")),
            "{f:?}"
        );
        assert!(!f.iter().any(|x| x.text.contains("No NGX call")), "{f:?}");

        let ran = format!(
            "{menu}[NRPRE] eval feat=1  render_subrect=1707x960 | a | b | c | d
             [NRPRE] 2b: SNIPPET CreateFeature(18, 1707x960) -> 0x00000001 handle=000001
             [NRPRE] 2c: NR Evaluate(HI) -> 0x00000001
"
        );
        let f = upstream_findings(&ran);
        assert!(
            f.iter()
                .any(|x| x.level == Level::Ok && x.text.contains("Neural rendering ran")),
            "{f:?}"
        );

        let failed = format!(
            "{menu}[NRPRE] eval feat=1  render_subrect=1707x960 | a | b | c | d
             [NRPRE] 2b: SNIPPET CreateFeature(18, 1707x960) -> 0xBAD00002 handle=0000000000000000
"
        );
        let f = upstream_findings(&failed);
        assert!(
            f.iter()
                .any(|x| x.level == Level::Bad && x.text.contains("0xBAD00002")),
            "{f:?}"
        );
    }

    /// Cyberpunk 2077 (#95): the pass "ran" — sixty evaluations in 95 seconds
    /// across 18 feature re-creations and four worksets. That is churn, not
    /// neural rendering, and Diagnose must not call it success.
    #[test]
    fn feature_churn_is_reported_as_a_failure_not_a_pass() {
        let (t, exe) = setup(false);
        let d = t.path();
        let mut log = String::from(
            "Initializing crosire's ReShade version '6.8.0'\nRegistered add-on \"DLSS 5 Neural Rendering\"\n",
        );
        for _ in 0..18 {
            log.push_str("DLSS5 Generic: feature 18 created via the signed snippet after DLSS/DLAA for NR input 1920x1080\n");
        }
        for _ in 0..6 {
            log.push_str(
                "DLSS5 Generic: multi-pass: a second NR workset is live; independent passes\n",
            );
        }
        log.push_str("DLSS5 Generic: inline feature 18 evaluation succeeded (count=60, NR input 1920x1080)\n");
        fs::write(d.join("ReShade.log"), &log).unwrap();
        let st = game::inspect(&exe).unwrap();
        let f = diagnose(&st);
        assert!(
            f.iter()
                .any(|x| x.level == Level::Bad && x.text.contains("created 18 times")),
            "{f:?}"
        );
        assert!(
            !f.iter().any(|x| x.text.starts_with("Neural rendering ran")),
            "{f:?}"
        );

        // One creation, sixty frames: the pass ran.
        fs::write(
            d.join("ReShade.log"),
            "Initializing crosire's ReShade version '6.8.0'\nRegistered add-on \"DLSS 5 Neural Rendering\"\n\
             DLSS5 Generic: feature 18 created via the signed snippet after DLSS/DLAA\n\
             DLSS5 Generic: inline feature 18 evaluation succeeded (count=60)\n",
        )
        .unwrap();
        let f = diagnose(&game::inspect(&exe).unwrap());
        assert!(
            f.iter().any(|x| x.text.starts_with("Neural rendering ran")),
            "{f:?}"
        );

        // Prey (#99): 93 creations from F6 mashing and slider drags, but the
        // pass had held for 600 straight evaluations. Not churn.
        let mut log = String::from(
            "Initializing crosire's ReShade version '6.8.0'
Registered add-on \"DLSS 5 Neural Rendering\"
             DLSS5 Generic: inline feature 18 evaluation succeeded (count=600, NR input 2880x1620)
",
        );
        for _ in 0..93 {
            log.push_str(
                "DLSS5 Generic: feature 18 created via the signed snippet after DLSS/DLAA
",
            );
        }
        fs::write(d.join("ReShade.log"), &log).unwrap();
        let f = diagnose(&game::inspect(&exe).unwrap());
        assert!(
            f.iter().any(|x| x.text.starts_with("Neural rendering ran")),
            "{f:?}"
        );
    }

    /// RDR2 (#97): Launcher.exe in the game folder loads dxgi.dll after the
    /// game exits, so ReShade.log is the launcher's and the game's session is
    /// ReShade.log1. Diagnose read the launcher's log and reported the add-on
    /// missing and no NGX call, then told the user to install for Launcher.exe.
    #[test]
    fn launcher_log_on_top_falls_back_to_the_games_log1() {
        let (t, exe) = setup(false);
        let d = t.path();
        fs::write(d.join(game::DLSS5_ADDON), b"a").unwrap();
        fs::write(
            d.join("ReShade.log"),
            "Initializing crosire's ReShade version '6.8.0' (64-bit) loaded from 'D:\\g\\dxgi.dll' into 'D:\\g\\Launcher.exe' (0x1) ...\n",
        )
        .unwrap();
        fs::write(
            d.join("ReShade.log1"),
            "Initializing crosire's ReShade version '6.8.0' (64-bit) loaded from 'D:\\g\\dxgi.dll' into 'D:\\g\\game.exe' (0x1) ...\n\
             Registered add-on \"DLSS 5 Neural Rendering\"\n",
        )
        .unwrap();
        let f = diagnose(&game::inspect(&exe).unwrap());
        assert!(f.iter().any(|x| x.text.contains("ReShade.log1")), "{f:?}");
        assert!(
            f.iter().any(|x| x.text.contains("add-on registered")),
            "{f:?}"
        );
        assert!(
            !f.iter().any(|x| x.text.contains("never registered")),
            "{f:?}"
        );
        assert!(
            !f.iter().any(|x| x.text.contains("different executables")),
            "{f:?}"
        );
    }

    /// "100% non-zero" contains "0% non-zero": every healthy feed log read as
    /// all-zero motion vectors. And the first probes run on a menu — the last
    /// one is the verdict (#99).
    #[test]
    fn mv_and_depth_probes_are_read_from_the_last_line_and_parsed() {
        let (t, exe) = setup(true);
        let d = t.path();
        fs::write(
            d.join("ReShade.log"),
            "Initializing crosire's ReShade version '6.8.0'\nRegistered add-on \"DLSS 5 Neural Rendering\"\n",
        )
        .unwrap();
        fs::write(
            d.join("dlss5-feed.log"),
            "[feed] feature ready: 3840x2160 DLAA\n\
             [feed] MV probe (centre 64x64, frame 600): mean |mv| 0.000 px, max 0.00 px, 0% non-zero  <-- DLSS is getting (almost) no motion vectors\n\
             [feed] Depth probe (4x 32x32, frame 600): min 0, max 0, mean 0, variance 0, 100% finite  <-- sampled depth is flat\n\
             [feed] MV probe (centre 64x64, frame 70800): mean |mv| 1.618 px, max 2.16 px, 100% non-zero\n\
             [feed] Depth probe (4x 32x32, frame 70800): min 0.0075, max 0.027, mean 0.016, variance 5.58e-05, 100% finite\n",
        )
        .unwrap();
        let f = diagnose(&game::inspect(&exe).unwrap());
        assert!(
            !f.iter()
                .any(|x| x.text.contains("Motion vectors are all zero")),
            "{f:?}"
        );
        assert!(!f.iter().any(|x| x.text.contains("Depth is flat")), "{f:?}");

        fs::write(
            d.join("dlss5-feed.log"),
            "[feed] feature ready: 3840x2160 DLAA\n\
             [feed] MV probe (centre 64x64, frame 600): mean |mv| 0.000 px, max 0.00 px, 0% non-zero  <-- DLSS is getting (almost) no motion vectors\n\
             [feed] Depth probe (4x 32x32, frame 600): min 0, max 0, mean 0, variance 0, 100% finite  <-- sampled depth is flat; inspect the depth debug view\n",
        )
        .unwrap();
        let f = diagnose(&game::inspect(&exe).unwrap());
        assert!(
            f.iter()
                .any(|x| x.text.contains("Motion vectors are all zero")),
            "{f:?}"
        );
        assert!(f.iter().any(|x| x.text.contains("Depth is flat")), "{f:?}");
    }

    /// ShortFuse's add-on is read by its own log lines: attached, model
    /// missing, and the evaluation count it prints when it detaches.
    #[test]
    fn shortfuse_log_is_read_by_its_own_lines() {
        let good = "RenoDX DLSS attached.
Init_Ext succeeded for device 1
detached signed snippet after 4312 successful evaluations
";
        let f = sf_findings(good);
        assert!(f.iter().any(|x| x.text.contains("4312")), "{f:?}");
        let nomodel = "RenoDX DLSS attached.
RenoDX DLSS could not attach the direct nvngx_dlssnr.dll runtime.
";
        assert!(sf_findings(nomodel)
            .iter()
            .any(|x| x.text.contains("nvngx_dlssnr.dll")));
        assert!(sf_findings(
            "Initializing crosire's ReShade
"
        )
        .iter()
        .any(|x| x.text.contains("never attached")));
    }

    /// Only ShortFuse's own detach line counts as an evaluation total.
    #[test]
    fn shortfuse_evaluations_come_only_from_the_detach_line() {
        let other = "RenoDX DLSS attached.
retry after 30
";
        assert!(!sf_findings(other).iter().any(|x| x.text.contains("ran:")));
    }

    /// The lines from the Kingdom Come: Deliverance II report in #118: the
    /// Feeder's own session renders at the output size, the game's own does not.
    #[test]
    fn a_game_side_dlss_session_beside_the_feeder_is_named() {
        let feeder = "16:13:41:349 [41212] | INFO  | [DLSS 5 Neural Rendering] DLSS5 Generic: NGX create contract: handle=0x1bbd3fa5b30 feature=1 (DLSS/DLAA) source=create flags=0x4a [MVLowRes DepthInverted AutoExposure] render=3440x1440 out=3440x1440";
        let game = "16:14:38:240 [41204] | INFO  | [DLSS 5 Neural Rendering] DLSS5 Generic: NGX create contract: handle=0x1ba974d9570 feature=1 (DLSS/DLAA) source=create flags=0x6b [IsHDR MVLowRes DepthInverted DoSharpening AutoExposure] render=2293x960 out=3440x1440";
        assert!(super::game_own_dlss_note(feeder, None).is_none());
        let note =
            super::game_own_dlss_note(&format!("{feeder}\n{game}"), Some("work_resolution=100\n"));
        assert!(note
            .as_deref()
            .is_some_and(|n| n.contains("2293x960") && n.contains("Force native DLSS")));
        // The Feeder's own session is smaller than the output when work_resolution is.
        assert!(super::game_own_dlss_note(game, Some("work_resolution=75\n")).is_none());
        assert!(super::game_own_dlss_note("nothing here", None).is_none());
    }
}
