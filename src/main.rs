#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// 翻译表编译期嵌入：locales/ 下所有 .yml（en / zh-CN），缺失 key 回退 en。
#[macro_use]
extern crate rust_i18n;

i18n!("locales", fallback = "en");

mod diagnose;
mod feeder_cfg;
mod game;
mod gpu;
mod gpupref;
mod gui;
mod i18n;
mod installer;
mod library;
mod logo;
mod net;
mod ngx;
mod pcgw;
mod quality_preset;
mod renodx;
mod report;
mod reshade_ini;
mod settings;
mod setup;
mod text;
mod theme;
mod update;

use std::io::Write;
use std::path::PathBuf;

/// `dlss5oneclick <GAME.exe | game folder> [--remove | --remove-all | --check | --diagnose | --report | --engine=opti|aio|mfg|mfgaddon | --renodx | --upstream | --imports | --ignore-anticheat | --mode=feeder|native | --api=dx11|dx12 | --addon=latest|rc|stable|4.55|<tag> | --rtxmfg | --ampere-mfg | --opti-tag=<tag> | --lang=en|zh-CN] | --update` runs headless; no args opens the GUI.
/// Read by the NVIDIA and AMD drivers from this exe's export table to choose
/// the discrete GPU for the whole process. Exported by the linker flags in
/// build.rs; the values themselves are what the drivers read (#32).
#[cfg(windows)]
#[no_mangle]
#[used]
pub static NvOptimusEnablement: u32 = 1;

#[cfg(windows)]
#[no_mangle]
#[used]
pub static AmdPowerXpressRequestHighPerformance: u32 = 1;

fn main() {
    install_panic_handler();
    update::cleanup_old();
    let args: Vec<String> = std::env::args().skip(1).collect();
    i18n::init(&args);
    if args.first().map(String::as_str) == Some("--fetch") {
        attach_parent_console();
        let (Some(url), Some(dest)) = (args.get(1), args.get(2)) else {
            eprintln!("{}", t!("main.fetch.usage"));
            std::process::exit(2);
        };
        let code = match net::client().and_then(|c| {
            net::download(
                &c,
                url,
                std::path::Path::new(dest),
                &t!("main.fetch.label"),
                &|p, m| {
                    print!("\r{p:3}% {m:<72}");
                    let _ = std::io::stdout().flush();
                },
            )
        }) {
            Ok(()) => {
                println!("\n{}", t!("main.fetch.done"));
                0
            }
            Err(e) => {
                eprintln!("\n{}", t!("main.error", err = format!("{e:#}")));
                1
            }
        };
        std::process::exit(code);
    }
    if args.iter().any(|a| a == "--list-games") {
        attach_parent_console();
        for g in library::scan() {
            let when = g
                .installed
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            println!(
                "{:<10} {:<12} {:<50} {} {}={:?}",
                when,
                g.store.label(),
                g.title.chars().take(50).collect::<String>(),
                g.dir.display(),
                t!("main.list_games.poster"),
                g.poster
            );
        }
        std::process::exit(0);
    }
    if args.iter().any(|a| a == "--posters") {
        // Support/diagnostic: decode every poster the scan resolved and report failures.
        attach_parent_console();
        let client = net::client().expect("http client");
        let (mut ok, mut bad) = (0usize, 0usize);
        for g in library::scan() {
            match library::poster_rgba(&client, &g.poster) {
                Some(img) => {
                    ok += 1;
                    println!(
                        "{}",
                        t!(
                            "main.posters.ok",
                            w = img.width(),
                            h = img.height(),
                            title = g.title.as_str(),
                            poster = format!("{:?}", g.poster)
                        )
                    );
                }
                None => {
                    bad += 1;
                    println!(
                        "{}",
                        t!(
                            "main.posters.fail",
                            title = g.title.as_str(),
                            poster = format!("{:?}", g.poster)
                        )
                    );
                }
            }
        }
        println!("{}", t!("main.posters.summary", ok = ok, bad = bad));
        std::process::exit(0);
    }
    if args.iter().any(|a| a == "--update") {
        attach_parent_console();
        std::process::exit(cli_update());
    }
    if args.iter().any(|a| a == "--imports") {
        attach_parent_console();
        let Some(first) = args.first().filter(|a| !a.starts_with('-')) else {
            eprintln!("{}", t!("main.imports.needs_target"));
            std::process::exit(1);
        };
        match game::resolve_target(&PathBuf::from(first)) {
            Ok((exe, _)) => {
                println!("{}", exe.display());
                println!(
                    "{}",
                    t!(
                        "main.imports.api_read",
                        api = game::detect_api(&exe).label()
                    )
                );
                let imports = game::pe_imports(&exe);
                println!("{}", t!("main.imports.imports", list = imports.join(", ")));
                for dll in ["d3d9.dll", "d3d11.dll", "d3d12.dll", "dxgi.dll"] {
                    let fns = game::pe_import_fns(&exe, dll);
                    if !fns.is_empty() {
                        println!(
                            "{}",
                            t!("main.imports.from_dll", dll = dll, list = fns.join(", "))
                        );
                    }
                }
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("{}", t!("main.error", err = format!("{e:#}")));
                std::process::exit(1);
            }
        }
    }
    if let Some(t) = args.iter().find_map(|a| a.strip_prefix("--opti-tag=")) {
        std::env::set_var(installer::OPTI_TAG_ENV, t);
    }
    if args.iter().any(|a| a == "--rtxmfg") {
        std::env::set_var(installer::RTXMFG_WITH_ENV, "1");
    }
    if args.iter().any(|a| a == "--ampere-mfg") {
        std::env::set_var(installer::AMPERE_MFG_ENV, "1");
    }
    if args.iter().any(|a| a == "--ignore-anticheat") {
        game::set_ignore_anticheat(true);
    }
    if let Some(m) = args.iter().find_map(|a| a.strip_prefix("--mode=")) {
        std::env::set_var(game::MODE_ENV, m);
        if game::mode_override().is_none() {
            eprintln!("{}", t!("main.err.mode"));
            std::process::exit(1);
        }
    }
    // The newest DLSS 5 add-on build includes release candidates unless the
    // user asked for stable builds only, here or in the GUI's advanced options.
    // `--addon=rc` asks for the candidates whatever the settings say.
    if args.iter().any(|a| a == "--addon=rc") {
        std::env::remove_var(installer::RENODX_STABLE_ENV);
    } else if args.iter().any(|a| a == "--addon=stable")
        || crate::settings::Settings::load().renodx_stable_only
    {
        std::env::set_var(installer::RENODX_STABLE_ENV, "1");
    }
    if let Some(a) = args
        .iter()
        .find_map(|a| a.strip_prefix("--addon="))
        .filter(|a| *a != "rc" && *a != "stable")
    {
        // "latest" is the newest stable build; a bare number becomes a tag.
        let v = if a.eq_ignore_ascii_case("latest") || a.starts_with("renodx-dlss5-") {
            a.to_owned()
        } else {
            format!("renodx-dlss5-{a}")
        };
        std::env::set_var(installer::RENODX_TAG_ENV, v);
    }
    if let Some(c) = args.iter().find_map(|a| a.strip_prefix("--consumer=")) {
        if !["sf", "shortfuse", "dlss5"]
            .iter()
            .any(|v| c.eq_ignore_ascii_case(v))
        {
            eprintln!("{}", t!("main.err.consumer"));
            std::process::exit(1);
        }
        std::env::set_var(installer::CONSUMER_ENV, c);
    }
    if let Some(a) = args.iter().find_map(|a| a.strip_prefix("--api=")) {
        std::env::set_var(game::API_ENV, a);
        if game::api_override().is_none() {
            eprintln!("{}", t!("main.err.api"));
            std::process::exit(1);
        }
    }
    if args.iter().any(|a| a == "--next-setup")
        && args.iter().any(|a| {
            a.starts_with("--engine=")
                || a == "--opti"
                || a == "--aio"
                || a == "--upstream"
                || a.starts_with("--addon=")
                || a.starts_with("--consumer=")
                || a == "--check"
                || a == "--remove"
                || a == "--remove-all"
                || a == "--diagnose"
                || a == "--report"
        })
        || args.iter().any(|a| a == "--next-setup")
            && (std::env::var_os(installer::RENODX_TAG_ENV).is_some()
                || std::env::var_os(installer::CONSUMER_ENV).is_some())
    {
        attach_parent_console();
        eprintln!("{}", t!("main.err.next_setup"));
        std::process::exit(1);
    }
    if let Some(first) = args.first().filter(|a| !a.starts_with('-')) {
        attach_parent_console();
        let code = cli(
            PathBuf::from(first),
            args.iter().any(|a| a == "--remove"),
            args.iter().any(|a| a == "--remove-all"),
            args.iter().any(|a| a == "--check"),
            args.iter().any(|a| a == "--diagnose"),
            args.iter().any(|a| a == "--report"),
            Choice {
                engine: if args.iter().any(|a| a == "--engine=opti" || a == "--opti") {
                    installer::Engine::Opti
                } else if args.iter().any(|a| a == "--engine=aio" || a == "--aio") {
                    installer::Engine::Aio
                } else if args
                    .iter()
                    .any(|a| a == "--engine=mfg" || a == "--mfg-only")
                {
                    installer::Engine::Mfg
                } else if args.iter().any(|a| a == "--engine=mfgaddon") {
                    installer::Engine::MfgAddon
                } else {
                    installer::Engine::ReShade
                },
                with_renodx: args.iter().any(|a| a == "--renodx"),
                upstream: args.iter().any(|a| a == "--upstream"),
                // Any explicit route choice turns the picker off, including a
                // build or consumer set in the environment.
                auto: !args.iter().any(|a| {
                    a.starts_with("--engine=")
                        || a == "--opti"
                        || a == "--aio"
                        || a == "--upstream"
                        || a.starts_with("--addon=")
                        || a.starts_with("--consumer=")
                }) && std::env::var_os(installer::RENODX_TAG_ENV).is_none()
                    && std::env::var_os(installer::CONSUMER_ENV).is_none(),
                next: args.iter().any(|a| a == "--next-setup"),
            },
        );
        std::process::exit(code);
    }
    if let Err(e) = gui::run() {
        // The release build has no console: say it in a box and leave a file (#23).
        let msg = format!("{e:#}");
        eprintln!("{}", t!("main.gui_error", msg = msg.as_str()));
        let log = write_error_log(&msg);
        report_gui_error(&t!(
            "main.gui_error_box",
            msg = msg.as_str(),
            log = log.display().to_string()
        ));
        std::process::exit(2);
    }
}

fn cli_update() -> i32 {
    match update::check() {
        Ok(None) => {
            println!("{}", t!("main.update.latest", ver = update::CURRENT));
            0
        }
        Ok(Some(av)) => {
            println!(
                "{}",
                t!(
                    "main.update.available",
                    from = update::CURRENT,
                    to = av.version.as_str()
                )
            );
            let progress = |pct: u8, msg: &str| {
                print!("\r{pct:3}% {msg:<72}");
                let _ = std::io::stdout().flush();
            };
            match update::download_and_swap(&av, &progress) {
                Ok(exe) => {
                    println!(
                        "{}",
                        t!(
                            "main.update.updated_to",
                            ver = av.version.as_str(),
                            path = exe.display().to_string()
                        )
                    );
                    0
                }
                Err(e) => {
                    eprintln!("\n{}", t!("main.error", err = format!("{e:#}")));
                    1
                }
            }
        }
        Err(e) => {
            eprintln!("{}", t!("main.error", err = format!("{e:#}")));
            1
        }
    }
}

/// What to install, as chosen on the command line.
struct Choice {
    engine: installer::Engine,
    with_renodx: bool,
    upstream: bool,
    /// Nothing about the route was asked for: the setup picker decides.
    auto: bool,
    /// `--next-setup`: move this game one rung down its ladder and install that.
    next: bool,
}

fn cli(
    target: PathBuf,
    remove: bool,
    remove_all: bool,
    check: bool,
    diagnose_only: bool,
    report: bool,
    choice: Choice,
) -> i32 {
    let Choice {
        engine,
        with_renodx,
        upstream,
        auto,
        next,
    } = choice;
    let (exe, candidates) = match game::resolve_target(&target) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{}", t!("main.error", err = format!("{e:#}")));
            return 1;
        }
    };
    if candidates.len() > 1 {
        let others: Vec<String> = candidates[1..]
            .iter()
            .map(|p| {
                p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        println!(
            "{}",
            t!(
                "main.using_with_others",
                exe = exe.display().to_string(),
                others = others.join(", ")
            )
        );
    } else if !candidates.is_empty() {
        println!("{}", t!("main.using", exe = exe.display().to_string()));
    }
    // The setup picker: the rung matching what is in the folder (the top one
    // when nothing is), or the next one down.
    let mut engine = engine;
    let mut upstream = upstream;
    let mut rung: Option<(usize, usize)> = None;
    let mut switching_engine = false;
    let mut with_renodx = with_renodx;
    if auto && !remove && !remove_all && !diagnose_only && !report {
        if let Ok(st) = game::inspect(&exe) {
            let ladder = setup::ladder(&st);
            if setup::hand_chosen(&st) {
                if next {
                    eprintln!("{}", t!("main.err.hand_chosen"));
                    return 1;
                }
                // Refresh what is there rather than replace it.
                if st.upstream {
                    upstream = true;
                } else if st.mfg_only {
                    engine = installer::Engine::MfgAddon;
                } else if st.rtxmfg {
                    engine = installer::Engine::Mfg;
                } else if st.aio && !st.opti {
                    engine = installer::Engine::Aio;
                } else if let Some(s) = setup::installed(&st) {
                    engine = setup::apply(&s, false);
                }
                println!(
                    "{}",
                    t!(
                        "main.setup_kept",
                        label = setup::hand_label(&st).unwrap_or_default()
                    )
                );
            } else if ladder.is_empty() {
                setup::clear();
                if next {
                    eprintln!("{}", t!("main.err.no_fit"));
                    return 1;
                }
            }
            if next && !setup::hand_chosen(&st) && setup::installed(&st).is_none() {
                eprintln!("{}", t!("main.err.nothing_installed"));
                return 1;
            }
            let pick = if setup::hand_chosen(&st) {
                None
            } else if next {
                match setup::next(&st) {
                    Some(p) => Some(p),
                    None => {
                        eprintln!(
                            "{}",
                            t!(
                                "main.err.last_setup",
                                label = setup::current(&st)
                                    .map(|s| setup::label(&s))
                                    .unwrap_or_default()
                            )
                        );
                        return 1;
                    }
                }
            } else {
                setup::current(&st).map(|s| (setup::level(&st), s))
            };
            if let Some((n, s)) = pick {
                let was = setup::current(&st).map(|c| c.engine);
                switching_engine = next && was != Some(s.engine);
                if switching_engine {
                    // Refuse before anything is removed: taking ReShade out
                    // would strip add-ons this tool did not put there.
                    let foreign = installer::foreign_addons(st.game_dir());
                    if !foreign.is_empty() {
                        eprintln!(
                            "{}",
                            t!(
                                "main.err.foreign_addons",
                                label = setup::label(&s),
                                list = foreign.join(", ")
                            )
                        );
                        return 1;
                    }
                    // The switch takes the RenoDX HDR mod out; it goes back in.
                    with_renodx = with_renodx || st.renodx_mod.is_some();
                }
                engine = setup::apply(&s, true);
                rung = Some((n, ladder.len()));
                println!(
                    "{}",
                    t!(
                        "main.setup_rung",
                        label = setup::label(&s),
                        n = n + 1,
                        total = ladder.len(),
                        reason = setup::reason(&st)
                    )
                );
            }
        }
    }
    if report {
        return match report::write_bundle(&exe) {
            Ok(p) => {
                println!(
                    "{}",
                    t!("main.report_written", path = p.display().to_string())
                );
                0
            }
            Err(e) => {
                eprintln!("{}", t!("main.error", err = format!("{e:#}")));
                1
            }
        };
    }
    if diagnose_only {
        return match diagnose::run(&exe) {
            Ok(findings) => {
                for f in &findings {
                    let tag = match f.level {
                        diagnose::Level::Ok => t!("main.diagnose.ok"),
                        diagnose::Level::Warn => t!("main.diagnose.warn"),
                        diagnose::Level::Bad => t!("main.diagnose.bad"),
                    };
                    println!("[{tag}] {}", text::tidy(&f.text));
                }
                if findings.iter().any(|f| f.level == diagnose::Level::Bad) {
                    1
                } else {
                    0
                }
            }
            Err(e) => {
                eprintln!("{}", t!("main.error", err = format!("{e:#}")));
                1
            }
        };
    }
    if check {
        pcgw::warm(&exe);
        return match game::inspect(&exe) {
            Ok(st) => {
                println!(
                    "{}",
                    t!(
                        "main.check.status",
                        exe = exe.display().to_string(),
                        bits = st.bitness,
                        api = st.api.label(),
                        mode = format!("{:?}", st.mode),
                        reshade = st.reshade,
                        headers = st.headers,
                        feeder = st.feeder,
                        lumenite = st.lumenite,
                        dlss5 = st.dlss5_addon,
                        dlssnr = st.dlssnr,
                        dlss = st.dlss,
                        bridge = st.bridge,
                        opti = st.opti,
                        gpu = st
                            .gpu
                            .as_ref()
                            .map(|(g, t)| format!("{} [{}]", g.name, t.label()))
                            .unwrap_or_else(|| t!("main.unknown_gpu").into_owned()),
                        complete = st.complete()
                    )
                );
                for p in &st.problems {
                    println!("  ! {}", text::tidy(p));
                }
                // No engine asked for and no pick: the plan follows what is in
                // the folder.
                let engine = match engine {
                    installer::Engine::ReShade if rung.is_none() && st.opti => {
                        installer::Engine::Opti
                    }
                    installer::Engine::ReShade if rung.is_none() && st.aio => {
                        installer::Engine::Aio
                    }
                    installer::Engine::ReShade if rung.is_none() && st.rtxmfg => {
                        installer::Engine::Mfg
                    }
                    installer::Engine::ReShade if rung.is_none() && st.mfg_only => {
                        installer::Engine::MfgAddon
                    }
                    e => e,
                };
                let names: Vec<String> = installer::plan_with(&st, engine, with_renodx, upstream)
                    .iter()
                    .map(|s| s.title().into_owned())
                    .collect();
                println!("{}", t!("main.check.plan", names = names.join(" -> ")));
                if st.re_engine {
                    println!(
                        "{}",
                        t!(
                            "main.check.re_engine",
                            state = if st.reframework {
                                t!("main.check.reframework_present")
                            } else {
                                t!("main.check.reframework_missing")
                            }
                        )
                    );
                }
                if gpupref::hybrid() {
                    // Which GPU Windows starts the process on decides whether NGX
                    // exists in it at all (#25).
                    println!(
                        "{}",
                        t!(
                            "main.check.hybrid",
                            adapters = gpupref::real_adapters().join(", "),
                            exe = exe.file_name().unwrap_or_default().to_string_lossy(),
                            pref = gpupref::get(&exe)
                                .unwrap_or_else(|| t!("main.check.gpu_pref_unset").into_owned())
                        )
                    );
                }
                if game::installed_by_tool(st.game_dir()) {
                    // Same comparison the Games page makes, so a stale install
                    // can be seen without opening the window.
                    let latest = net::client()
                        .map(|c| installer::Latest::fetch(&c))
                        .unwrap_or_default();
                    match setup::stale(&st, &latest).as_slice() {
                        [] => println!("{}", t!("main.check.all_current")),
                        stale => {
                            println!("{}", t!("main.check.out_of_date"));
                            for s in stale {
                                println!("    {s}");
                            }
                        }
                    }
                }
                if let Some(m) = &st.renodx_mod {
                    println!("{}", t!("main.check.renodx_installed", mod_name = m));
                }
                if !st.foreign_renodx.is_empty() {
                    println!(
                        "{}",
                        t!(
                            "main.check.renodx_foreign",
                            list = st.foreign_renodx.join(", ")
                        )
                    );
                }
                match net::client().and_then(|c| renodx::lookup(&c, &exe)) {
                    Ok(Some(m)) => println!(
                        "{}",
                        t!(
                            "main.check.renodx_available",
                            title = m.title,
                            file = m.file,
                            status = m.status_label(),
                            source = m.source_label(),
                            note = if m.note.is_empty() {
                                String::new()
                            } else {
                                format!(" | {}", m.note)
                            }
                        )
                    ),
                    Ok(None) => println!("{}", t!("main.check.renodx_none")),
                    Err(e) => {
                        println!("{}", t!("main.check.renodx_failed", err = format!("{e:#}")))
                    }
                }
                0
            }
            Err(e) => {
                eprintln!("{}", t!("main.error", err = format!("{e:#}")));
                1
            }
        };
    }
    if remove_all {
        return match installer::uninstall_all(&exe) {
            Ok((list, kept)) => {
                for f in list {
                    println!("{}", t!("main.removed", file = f));
                }
                if let Some(k) = kept {
                    println!("{k}");
                }
                0
            }
            Err(e) => {
                eprintln!("{}", t!("main.error", err = format!("{e:#}")));
                1
            }
        };
    }
    if remove {
        return match installer::uninstall(&exe) {
            Ok(list) => {
                for f in list {
                    println!("{}", t!("main.removed", file = f));
                }
                0
            }
            Err(e) => {
                eprintln!("{}", t!("main.error", err = format!("{e:#}")));
                1
            }
        };
    }
    let progress = |pct: u8, msg: &str| {
        print!("\r{pct:3}% {msg:<72}");
        let _ = std::io::stdout().flush();
    };
    let step = move |i: usize, n: usize, name: &str, state: installer::StepState, detail: &str| {
        use installer::StepState::*;
        match state {
            Start => println!("\n[{}/{}] {name}", i + 1, n),
            Done => println!("\n      {}", t!("main.step.ok", detail = detail)),
            Error => println!("\n      {}", t!("main.step.failed", detail = detail)),
        }
    };
    if switching_engine {
        // ReShade and OptiScaler both load as dxgi.dll: the old one goes first.
        match installer::uninstall_all(&exe) {
            Ok((list, _)) => println!("{}", t!("main.removed_previous", list = list.join(", "))),
            Err(e) => {
                eprintln!("{}", t!("main.err.remove_previous", err = format!("{e:#}")));
                return 1;
            }
        }
    }
    let s = settings::Settings::load();
    match installer::run_all_with(
        &exe,
        engine,
        with_renodx,
        upstream,
        installer::InstallOpts {
            quality: s.quality_choice(),
            overrides: s.quality_overrides(),
        },
        &progress,
        &step,
    ) {
        Ok(_) => {
            if engine == installer::Engine::Opti {
                println!("\n{}", t!("main.done.opti"));
            } else if engine == installer::Engine::MfgAddon {
                println!("\n{}", t!("main.done.mfg_addon"));
            } else if engine == installer::Engine::Mfg {
                println!("\n{}", t!("main.done.mfg"));
            } else if engine == installer::Engine::Aio {
                println!("\n{}", t!("main.done.aio"));
            } else if installer::consumer() == installer::Consumer::ShortFuse
                && game::inspect(&exe).is_ok_and(|s| s.sf)
            {
                println!("\n{}", t!("main.done.shortfuse"));
            } else {
                println!("\n{}", t!("main.done.default"));
            }
            0
        }
        Err(e) => {
            eprintln!("\n{}", t!("main.error", err = format!("{e:#}")));
            if switching_engine {
                eprintln!("{}", t!("main.err.previous_removed"));
            }
            1
        }
    }
}

/// Where a failure that has no console to print to is recorded (#23, #32).
fn error_log_path() -> std::path::PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("dlss5oneclick")
        .join("gui-error.txt")
}

fn write_error_log(msg: &str) -> std::path::PathBuf {
    let log = error_log_path();
    if let Some(p) = log.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let _ = std::fs::write(
        &log,
        format!(
            "DLSS5oneclick {}
{msg}
",
            env!("CARGO_PKG_VERSION")
        ),
    );
    log
}

/// A release build has `windows_subsystem = "windows"`, so a panic writes to a
/// stderr nobody can see and the process simply vanishes -- which is exactly
/// what two reporters described as "nothing happens" (#23, #32). Record it and
/// say so in a box instead.
fn install_panic_handler() {
    std::panic::set_hook(Box::new(|info| {
        let where_ = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| t!("main.panic.unknown_location").into_owned());
        let what = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panic".into());
        let msg = t!(
            "main.panic.msg",
            loc = where_.as_str(),
            what = what.as_str()
        )
        .into_owned();
        eprintln!("{msg}");
        let log = write_error_log(&msg);
        report_gui_error(&t!(
            "main.panic.box",
            msg = msg.as_str(),
            log = log.display().to_string()
        ));
    }));
}

#[cfg(windows)]
fn report_gui_error(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let (t, c) = (wide(text), wide("DLSS5oneclick"));
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            t.as_ptr(),
            c.as_ptr(),
            MB_OK | MB_ICONERROR,
        )
    };
}
#[cfg(not(windows))]
fn report_gui_error(_text: &str) {}

/// Release builds hide the console; when launched from a terminal, reattach so CLI output shows.
fn attach_parent_console() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

#[cfg(test)]
mod tests {
    /// The crash log has to name the version, or a report cannot be matched to
    /// a build (#23, #32).
    #[test]
    fn error_log_records_version_and_message() {
        let p = super::write_error_log("panic at src/gui.rs:12: test");
        let body = std::fs::read_to_string(&p).unwrap();
        assert!(body.starts_with(&format!("DLSS5oneclick {}", env!("CARGO_PKG_VERSION"))));
        assert!(body.contains("panic at src/gui.rs:12: test"));
        let _ = std::fs::remove_file(&p);
    }
}
