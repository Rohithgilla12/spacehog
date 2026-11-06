use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use spacehog_core::Index;
use spacehog_core::cleanup::{ExecutionReport, Outcome};
use spacehog_core::config::Config;
use spacehog_core::proposals::{Actor, ProposalStatus, ProposeRequest};
use spacehog_core::report::{self, TopQuery};
use spacehog_core::rules::RuleSet;
use spacehog_tui::app::Overlay;
use spacehog_tui::categories::Row;
use spacehog_tui::cleanup::{Phase, Tab};
use spacehog_tui::keys::View;
use spacehog_tui::theme::Tone;
use spacehog_tui::{App, Ctx, Theme, TuiOptions, ui};

struct Fixture {
    _dir: tempfile::TempDir,
    home: PathBuf,
    state: PathBuf,
}

fn write(path: &Path, bytes: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![b'.'; bytes]).unwrap();
}

fn age(path: &Path, days: u64) {
    let t = SystemTime::now() - Duration::from_secs(days * 86_400);
    fs::File::open(path).unwrap().set_modified(t).unwrap();
}

/// Keeps journal writes and trashed items out of the real state dir and
/// `~/.Trash`. Set once per test binary, before any test reads them.
fn isolate() -> PathBuf {
    static ONCE: std::sync::Once = std::sync::Once::new();
    let base = std::env::temp_dir().join(format!("spacehog-tui-test-{}", std::process::id()));
    ONCE.call_once(|| {
        // SAFETY: runs once, before any test thread reads these variables.
        unsafe {
            std::env::set_var("SPACEHOG_STATE_DIR", base.join("state"));
            std::env::set_var("SPACEHOG_TRASH_DIR", base.join("trash"));
        }
    });
    base.join("trash")
}

/// A fake home covering several buckets, with one stale project.
fn fixture() -> Fixture {
    isolate();
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    let home = root.join("home");
    let state = root.join("state");
    let files: &[(&str, usize)] = &[
        (".npm/_cacache/index", 40_000),
        ("Library/Caches/Homebrew/node.tar.gz", 30_000),
        (
            "Library/Containers/com.docker.docker/Data/vms/0/data/Docker.raw",
            90_000,
        ),
        ("Downloads/setup.dmg", 60_000),
        ("Movies/clip.mp4", 50_000),
        ("code/web/package.json", 50),
        ("code/web/pnpm-lock.yaml", 100),
        ("code/web/node_modules/react/index.js", 70_000),
        ("code/web/.git/HEAD", 20),
        ("code/web/src/main.ts", 300),
        ("code/rs/Cargo.toml", 100),
        ("code/rs/target/debug/app", 120_000),
        ("code/old/package.json", 50),
        ("code/old/node_modules/x/index.js", 20_000),
    ];
    for (p, n) in files {
        write(&home.join(p), *n);
    }
    for p in [
        "code/old/package.json",
        "code/old/node_modules/x/index.js",
        "code/old/node_modules/x",
        "code/old/node_modules",
        "code/old",
    ] {
        age(&home.join(p), 200);
    }
    Fixture {
        _dir: dir,
        home,
        state,
    }
}

impl Fixture {
    fn ctx(&self) -> Ctx {
        let db = self.state.join("index.db");
        Ctx {
            index: Index::open(&db).unwrap(),
            rules: Arc::new(RuleSet::default_rules(&self.home).unwrap()),
            config: Config::default(),
            db,
            lock_dir: self.state.clone(),
            home: self.home.clone(),
        }
    }

    fn scanned_app(&self) -> App {
        let mut app = self.app(&TuiOptions {
            root: Some(self.home.clone()),
            ..Default::default()
        });
        wait_for_scan(&mut app);
        assert!(
            app.session.is_some(),
            "scan produced no session: {:?}",
            app.flash
        );
        app
    }

    fn app(&self, opts: &TuiOptions) -> App {
        App::new(self.ctx(), Theme::new(Tone::Dark, true, false), opts).unwrap()
    }
}

fn wait_for_scan(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while app.scan.is_some() {
        assert!(Instant::now() < deadline, "scan did not finish");
        std::thread::sleep(Duration::from_millis(20));
        app.tick().unwrap();
    }
    let flash = app.flash.as_deref().unwrap_or("");
    assert!(
        !flash.contains("failed") && !flash.contains("locked"),
        "scan failed: {flash}"
    );
}

fn screen(app: &mut App, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer();
    let mut out = String::new();
    for y in 0..h {
        for x in 0..w {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

fn press(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
        .unwrap();
}

fn type_str(app: &mut App, s: &str) {
    for c in s.chars() {
        press(app, KeyCode::Char(c));
    }
}

fn breadcrumb(screen: &str) -> &str {
    screen.lines().nth(1).unwrap()
}

#[test]
fn overview_shows_legend_and_top_offenders_at_80x24_and_wide() {
    let fx = fixture();
    let mut app = fx.scanned_app();
    let ctx = fx.ctx();
    let scan = ctx.index.latest_scan(None).unwrap().unwrap();
    let largest = &report::top(
        &ctx.index,
        &ctx.rules,
        scan.id,
        &TopQuery {
            limit: 1,
            ..Default::default()
        },
    )
    .unwrap()[0];
    assert!(largest.path.ends_with("code/rs/target"), "{}", largest.path);

    let small = screen(&mut app, 80, 24);
    assert!(small.contains("Top offenders"), "{small}");
    assert!(
        small.contains("Dev builds"),
        "legend lists the bucket:\n{small}"
    );
    assert!(small.contains("Docker/VMs"), "{small}");
    assert!(small.contains(&largest.size_human), "{small}");
    assert!(small.contains("1 Overview"), "{small}");
    assert!(small.contains("no proposals"), "status strip:\n{small}");

    let wide = screen(&mut app, 160, 40);
    assert!(wide.contains("~/code/rs/target"), "{wide}");
    assert!(wide.contains("~/code/old/node_modules"), "{wide}");
    assert!(
        wide.lines()
            .any(|l| l.contains("~/code/old/node_modules") && l.contains("6mo")),
        "stale badge on the 200-day-old project:\n{wide}"
    );
    assert!(wide.contains("Package caches"), "{wide}");
}

#[test]
fn narrow_terminal_drops_the_side_panel() {
    let fx = fixture();
    let mut app = fx.scanned_app();
    let s = screen(&mut app, 70, 24);
    assert!(s.contains("Top offenders"), "{s}");
    assert!(
        !s.contains("Docker/VMs  "),
        "legend is gone below 80 cols:\n{s}"
    );
}

#[test]
fn bucket_selection_filters_top_offenders() {
    let fx = fixture();
    let mut app = fx.scanned_app();
    let buckets: Vec<String> = app
        .session
        .as_ref()
        .unwrap()
        .overview
        .buckets
        .iter()
        .map(|b| b.id.clone())
        .collect();
    let target = buckets.iter().position(|b| b == "docker-vms").unwrap();
    for _ in 0..=target {
        press(&mut app, KeyCode::Right);
    }
    let top = &app.session.as_ref().unwrap().overview.top;
    assert!(!top.is_empty());
    assert!(top.iter().all(|i| i.bucket == "docker-vms"), "{top:?}");
    let s = screen(&mut app, 120, 30);
    assert!(s.contains("Top offenders · Docker/VMs"), "{s}");
    assert!(!s.contains("~/code/rs/target"), "{s}");
}

#[test]
fn explorer_descends_and_ascends_with_breadcrumb() {
    let fx = fixture();
    let mut app = fx.scanned_app();
    press(&mut app, KeyCode::Char('2'));
    assert_eq!(app.view, View::Explorer);

    let s = screen(&mut app, 80, 24);
    let first = app
        .session
        .as_ref()
        .unwrap()
        .explorer
        .selected()
        .unwrap()
        .name
        .clone();
    assert!(
        breadcrumb(&s).starts_with("~   ▸ "),
        "names the listed folder, then the selection:\n{s}"
    );
    assert!(breadcrumb(&s).contains(&first), "{s}");

    while app
        .session
        .as_ref()
        .unwrap()
        .explorer
        .selected()
        .unwrap()
        .name
        != "code"
    {
        press(&mut app, KeyCode::Down);
    }
    press(&mut app, KeyCode::Enter);
    let s = screen(&mut app, 80, 24);
    assert!(
        breadcrumb(&s).contains("~ › code   ▸ rs"),
        "largest child of code first:\n{s}"
    );

    press(&mut app, KeyCode::Right);
    let s = screen(&mut app, 120, 30);
    assert!(breadcrumb(&s).contains("~ › code › rs   ▸ target"), "{s}");
    assert!(
        s.lines()
            .any(|l| l.contains("target/") && l.contains("Dev builds")),
        "{s}"
    );

    press(&mut app, KeyCode::Left);
    press(&mut app, KeyCode::Backspace);
    let s = screen(&mut app, 80, 24);
    assert_eq!(breadcrumb(&s).trim_end(), "~   ▸ code", "{s}");
}

#[test]
fn explorer_wide_with_preview_pane() {
    let fx = fixture();
    let mut app = fx.scanned_app();
    press(&mut app, KeyCode::Char('2'));
    press(&mut app, KeyCode::Char('p'));
    let s = screen(&mut app, 160, 40);
    let first = app
        .session
        .as_ref()
        .unwrap()
        .explorer
        .selected()
        .unwrap()
        .name
        .clone();
    assert!(
        s.contains(&format!(" {first} ")),
        "preview titled by the selected folder:\n{s}"
    );
    assert!(s.contains("▀"), "half-block tiles drawn:\n{s}");
    let narrow = screen(&mut app, 79, 24);
    assert!(
        !narrow.contains("▀"),
        "preview dropped below 80 cols:\n{narrow}"
    );
}

#[test]
fn enter_on_overview_reveals_item_in_explorer() {
    let fx = fixture();
    let mut app = fx.scanned_app();
    let want = app.session.as_ref().unwrap().overview.top[0].path.clone();
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.view, View::Explorer);
    let got = app
        .session
        .as_ref()
        .unwrap()
        .explorer
        .selected()
        .unwrap()
        .path
        .clone();
    assert_eq!(got, want);
    let s = screen(&mut app, 100, 30);
    assert!(breadcrumb(&s).contains("code › rs   ▸ target"), "{s}");
}

#[test]
fn view_switching_and_back() {
    let fx = fixture();
    let mut app = fx.scanned_app();
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.view, View::Explorer);
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.view, View::Treemap, "tab visits every view in order");
    press(&mut app, KeyCode::Char('3'));
    assert_eq!(app.view, View::Treemap);
    assert!(screen(&mut app, 80, 24).contains("depth 2"));
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.view, View::Overview);
    press(&mut app, KeyCode::Char('2'));
    press(&mut app, KeyCode::Char('q'));
    assert_eq!(app.view, View::Overview);
    assert!(!app.quit);
    press(&mut app, KeyCode::Char('q'));
    assert!(app.quit);
}

#[test]
fn d_queues_the_selection_once() {
    let fx = fixture();
    let mut app = fx.scanned_app();
    press(&mut app, KeyCode::Down);
    let second = PathBuf::from(&app.session.as_ref().unwrap().overview.top[1].path);
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Char('d'));
    assert_eq!(app.queue, vec![second.clone()]);
    let s = screen(&mut app, 100, 30);
    assert!(s.contains("1 queued"), "{s}");

    press(&mut app, KeyCode::Char('2'));
    explorer_select(&mut app, "Downloads");
    press(&mut app, KeyCode::Char('d'));
    assert_eq!(app.queue, vec![second, fx.home.join("Downloads")]);
}

#[test]
fn toggles_change_what_is_listed() {
    let fx = fixture();
    let mut app = fx.scanned_app();
    press(&mut app, KeyCode::Char('s'));
    let top = &app.session.as_ref().unwrap().overview.top;
    assert_eq!(top.len(), 1, "{top:?}");
    assert!(top[0].path.ends_with("code/old/node_modules"));
    press(&mut app, KeyCode::Char('s'));

    press(&mut app, KeyCode::Char('h'));
    let top = &app.session.as_ref().unwrap().overview.top;
    let home = fx.home.display().to_string();
    assert!(!top.is_empty());
    assert!(
        top.iter().all(|i| !i.path[home.len()..].contains("/.")),
        "{top:?}"
    );
    press(&mut app, KeyCode::Char('2'));
    let s = screen(&mut app, 100, 30);
    assert!(!s.contains(".npm"), "{s}");
    assert!(s.contains("no hidden"), "{s}");
}

#[test]
fn filter_narrows_explorer_rows() {
    let fx = fixture();
    let mut app = fx.scanned_app();
    press(&mut app, KeyCode::Char('2'));
    press(&mut app, KeyCode::Char('/'));
    type_str(&mut app, "mov");
    press(&mut app, KeyCode::Enter);
    assert!(app.overlay.is_none());
    let s = screen(&mut app, 100, 30);
    assert!(s.contains("Movies/"), "{s}");
    assert!(!s.contains("Downloads/"), "{s}");
    press(&mut app, KeyCode::Esc);
    let s = screen(&mut app, 100, 30);
    assert!(s.contains("Downloads/"), "esc clears the filter:\n{s}");
}

#[test]
fn help_palette_and_info_overlays() {
    let fx = fixture();
    let mut app = fx.scanned_app();
    press(&mut app, KeyCode::Char('?'));
    let s = screen(&mut app, 80, 24);
    assert!(s.contains("Keys · Overview"), "{s}");
    assert!(s.contains("Add to cleanup queue"), "{s}");
    press(&mut app, KeyCode::Esc);
    assert!(app.overlay.is_none());

    press(&mut app, KeyCode::Char('i'));
    let s = screen(&mut app, 100, 30);
    assert!(s.contains("Why is this here"), "{s}");
    assert!(s.contains("regenerable via rule"), "{s}");
    assert!(s.contains("cargo"), "regen hint:\n{s}");
    assert!(
        s.contains("from its project ~/code/rs"),
        "home paths shortened:\n{s}"
    );
    press(&mut app, KeyCode::Esc);

    press(&mut app, KeyCode::Char(':'));
    type_str(&mut app, "explorer");
    let s = screen(&mut app, 80, 24);
    assert!(s.contains("Go to Explorer"), "{s}");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.view, View::Explorer);
    assert!(app.overlay.is_none());
}

#[test]
fn scan_modal_backgrounds_and_reloads() {
    let fx = fixture();
    let mut app = fx.scanned_app();
    let before = app.session.as_ref().unwrap().snap.scan.id;
    press(&mut app, KeyCode::Char('r'));
    assert!(matches!(app.overlay, Some(Overlay::Scan)));
    let s = screen(&mut app, 80, 24);
    assert!(s.contains("Scanning"), "{s}");
    assert!(s.contains("files"), "{s}");
    assert!(s.contains("walking"), "phase stepper:\n{s}");
    assert!(s.contains("Largest folders so far"), "{s}");
    assert!(s.contains("run in background"), "{s}");
    let wide = screen(&mut app, 160, 40);
    assert!(wide.contains("files/s"), "{wide}");

    press(&mut app, KeyCode::Esc);
    assert!(app.overlay.is_none());
    assert!(app.scan.is_some());
    let s = screen(&mut app, 100, 30);
    assert!(s.contains("scanning…"), "background badge:\n{s}");
    wait_for_scan(&mut app);
    assert!(app.session.as_ref().unwrap().snap.scan.id > before);
    assert!(app.flash.as_deref().unwrap_or("").starts_with("scanned"));
}

#[test]
fn first_run_shows_root_picker_then_scans() {
    let fx = fixture();
    let mut app = fx.app(&TuiOptions::default());
    assert!(app.session.is_none());
    let s = screen(&mut app, 80, 24);
    assert!(s.contains("No index yet"), "{s}");
    let home = spacehog_core::paths::home_dir().display().to_string();
    assert!(s.contains(&format!("› {home}")), "defaults to $HOME:\n{s}");

    app.picker = tui_input::Input::new(fx.home.display().to_string());
    press(&mut app, KeyCode::Enter);
    assert!(matches!(app.overlay, Some(Overlay::Scan)));
    wait_for_scan(&mut app);
    assert!(app.session.is_some());
    assert!(screen(&mut app, 80, 24).contains("Top offenders"));
}

fn session(app: &App) -> &spacehog_tui::app::Session {
    app.session.as_ref().unwrap()
}

fn explorer_select(app: &mut App, name: &str) {
    for _ in 0..40 {
        if session(app).explorer.selected().unwrap().name == name {
            return;
        }
        press(app, KeyCode::Down);
    }
    panic!("{name} not in the Explorer");
}

fn overview_select(app: &mut App, suffix: &str) {
    press(app, KeyCode::Char('g'));
    for _ in 0..60 {
        if session(app)
            .overview
            .selected_item()
            .is_some_and(|i| i.path.ends_with(suffix))
        {
            return;
        }
        press(app, KeyCode::Down);
    }
    panic!("{suffix} not in the top offenders");
}

fn queue_from_overview(app: &mut App, suffix: &str) -> PathBuf {
    overview_select(app, suffix);
    let path = PathBuf::from(&session(app).overview.selected_item().unwrap().path);
    press(app, KeyCode::Char('d'));
    assert!(app.queue.contains(&path), "{:?}", app.flash);
    path
}

/// Ticks until the run finishes and the bytes-freed counter stops moving.
fn wait_for_cleanup(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        assert!(Instant::now() < deadline, "cleanup did not finish");
        app.tick().unwrap();
        if let Phase::Done(d) = &app.cleanup.phase
            && d.counter.settled()
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn report(app: &App) -> ExecutionReport {
    match &app.cleanup.phase {
        Phase::Done(d) => d.outcome.clone().expect("executor ran"),
        _ => panic!("cleanup is not done"),
    }
}

fn agent_proposal(fx: &Fixture, rel: &[&str], rationale: &str) -> String {
    let ctx = fx.ctx();
    ctx.index
        .propose(
            &ctx.rules,
            &ctx.config,
            Actor::Agent,
            &ProposeRequest {
                paths: rel.iter().map(|p| fx.home.join(p)).collect(),
                rationale: rationale.into(),
                agent: Some("claude-code".into()),
                mode: None,
            },
        )
        .unwrap()
        .proposal_id
        .expect("proposal accepted")
}

fn open_proposals_tab(app: &mut App) {
    press(app, KeyCode::Char('5'));
    press(app, KeyCode::Right);
    assert_eq!(app.cleanup.tab, Tab::Proposals);
}

#[test]
fn categories_list_buckets_rules_and_the_selection_footer() {
    let fx = fixture();
    let mut app = fx.scanned_app();
    press(&mut app, KeyCode::Char('4'));
    let small = screen(&mut app, 80, 24);
    assert!(small.contains("Dev builds"), "{small}");
    assert!(small.contains("Cargo target dir"), "rule row:\n{small}");
    assert!(
        small.contains("total") && small.contains("stale"),
        "{small}"
    );

    while !matches!(
        session(&app).categories.current(),
        Some(Row::Rule(g, r)) if session(&app).categories.groups[g].rules[r].summary.rule_id == "node-modules"
    ) {
        press(&mut app, KeyCode::Down);
    }
    press(&mut app, KeyCode::Enter);
    let wide = screen(&mut app, 160, 40);
    assert!(
        wide.contains("~/code/web/node_modules"),
        "expanded rule lists paths:\n{wide}"
    );
    assert!(wide.contains("~/code/old/node_modules"), "{wide}");

    press(&mut app, KeyCode::Char(' '));
    let wide = screen(&mut app, 160, 40);
    assert!(
        wide.contains("selected: 2 items · ") && wide.contains("all regenerable"),
        "space on a rule selects its paths:\n{wide}"
    );

    press(&mut app, KeyCode::Char('s'));
    let stale = screen(&mut app, 120, 30);
    assert!(stale.contains("stale only"), "{stale}");
    assert!(
        !stale.contains("Docker/VMs"),
        "nothing stale there:\n{stale}"
    );
    press(&mut app, KeyCode::Char('s'));

    press(&mut app, KeyCode::Char('a'));
    let s = screen(&mut app, 120, 30);
    let footer = session(&app).categories.footer().unwrap();
    assert!(
        footer.contains("regenerable") && footer.contains("cache"),
        "{footer}"
    );
    assert!(s.contains(&footer), "mixed safety classes are named:\n{s}");

    press(&mut app, KeyCode::Char('d'));
    assert!(
        app.queue.iter().any(|p| p.ends_with("code/rs/target")),
        "{:?}",
        app.queue
    );
    assert!(
        !app.queue.iter().any(|p| p.ends_with("Movies/clip.mp4")),
        "protected items never reach the queue"
    );
    assert!(
        app.flash.as_deref().unwrap_or("").contains("refused"),
        "{:?}",
        app.flash
    );
}

#[test]
fn d_on_a_protected_item_names_the_reason_instead() {
    let fx = fixture();
    let mut app = fx.scanned_app();
    overview_select(&mut app, "Movies/clip.mp4");
    press(&mut app, KeyCode::Char('d'));
    assert!(app.queue.is_empty());
    let flash = app.flash.clone().unwrap_or_default();
    assert!(
        flash.contains("not queued") && flash.contains("protected"),
        "{flash}"
    );
    let s = screen(&mut app, 120, 30);
    assert!(s.contains("protected"), "{s}");

    press(&mut app, KeyCode::Char('2'));
    explorer_select(&mut app, "code");
    press(&mut app, KeyCode::Char('d'));
    assert!(app.queue.is_empty(), "an unclaimed folder is protected too");
}

#[test]
fn cleanup_shows_the_queue_and_agent_proposals_and_polls_fast() {
    let fx = fixture();
    let mut app = fx.scanned_app();
    queue_from_overview(&mut app, "code/web/node_modules");
    press(&mut app, KeyCode::Char('5'));
    for (w, h) in [(80, 24), (160, 40)] {
        let s = screen(&mut app, w, h);
        assert!(s.contains("Queue (1)"), "{s}");
        assert!(s.contains("~/code/web/node_modules"), "{s}");
        assert!(s.contains("regenerable"), "safety class:\n{s}");
        assert!(s.contains("is rebuilt by its tool"), "why safe:\n{s}");
    }

    open_proposals_tab(&mut app);
    assert!(screen(&mut app, 80, 24).contains("No proposals"));
    let id = agent_proposal(
        &fx,
        &[".npm/_cacache"],
        "npm refills this cache on the next install",
    );
    std::thread::sleep(spacehog_tui::app::CLEANUP_POLL + Duration::from_millis(50));
    app.tick().unwrap();
    for (w, h) in [(80, 24), (160, 40)] {
        let s = screen(&mut app, w, h);
        assert!(s.contains("claude-code"), "agent name:\n{s}");
        assert!(s.contains("npm refills this cache"), "rationale:\n{s}");
        assert!(s.contains("~/.npm/_cacache"), "{s}");
        assert!(s.contains("cache that refills on demand"), "why safe:\n{s}");
        assert!(s.contains(&id), "{s}");
    }
    assert!(
        screen(&mut app, 200, 50)
            .contains("regen: re-downloaded on install (npm cache clean --force)"),
        "regen hint"
    );
}

#[test]
fn approve_and_reject_with_a_reason_the_agent_can_read() {
    let fx = fixture();
    let mut app = fx.scanned_app();
    let keep = agent_proposal(&fx, &[".npm/_cacache"], "cache");
    let drop = agent_proposal(&fx, &["Library/Caches/Homebrew"], "brew cache");
    open_proposals_tab(&mut app);
    while app.cleanup.selected_proposal().unwrap().id != keep {
        press(&mut app, KeyCode::Down);
    }
    press(&mut app, KeyCode::Enter);
    let ctx = fx.ctx();
    assert_eq!(
        ctx.index.proposal(&keep).unwrap().unwrap().status,
        ProposalStatus::Approved
    );
    assert!(screen(&mut app, 100, 30).contains("approved"));

    while app.cleanup.selected_proposal().unwrap().id != drop {
        press(&mut app, KeyCode::Down);
    }
    press(&mut app, KeyCode::Char('x'));
    let s = screen(&mut app, 80, 24);
    assert!(s.contains("Reject proposal"), "{s}");
    type_str(&mut app, "keep it, I am offline this week");
    press(&mut app, KeyCode::Enter);
    let p = ctx.index.proposal(&drop).unwrap().unwrap();
    assert_eq!(p.status, ProposalStatus::Rejected);
    assert_eq!(p.reason.as_deref(), Some("keep it, I am offline this week"));
    assert!(app.cleanup.proposals.iter().all(|p| p.id != drop));
}

#[test]
fn executing_the_queue_trashes_into_the_trash_dir_and_counts_bytes() {
    let fx = fixture();
    let trash = isolate();
    let mut app = fx.scanned_app();
    let path = queue_from_overview(&mut app, "code/web/node_modules");
    press(&mut app, KeyCode::Char('5'));
    press(&mut app, KeyCode::Char('D'));
    assert!(matches!(app.cleanup.phase, Phase::Running(_)));
    wait_for_cleanup(&mut app);
    let r = report(&app);
    assert_eq!(r.results.len(), 1);
    let landed = PathBuf::from(
        r.results[0]
            .trashed_to
            .clone()
            .expect("renamed into the trash dir"),
    );
    assert!(landed.starts_with(&trash), "{landed:?} outside {trash:?}");
    assert!(landed.join("react/index.js").exists());
    assert!(!path.exists(), "left the fixture");
    assert_eq!(r.bytes_freed, r.results[0].size);

    for (w, h) in [(80, 24), (160, 40)] {
        let s = screen(&mut app, w, h);
        assert!(s.contains(&format!("{} freed", r.bytes_freed_human)), "{s}");
        assert!(s.contains(&r.journal_id), "{s}");
        assert!(s.contains("undo"), "{s}");
    }
    assert!(app.queue.is_empty(), "cleaned items leave the queue");

    press(&mut app, KeyCode::Char('u'));
    assert!(path.join("react/index.js").exists(), "undo puts it back");
    assert!(app.flash.as_deref().unwrap_or("").contains("restored 1"));
}

#[test]
fn permanent_mode_refuses_until_the_exact_total_is_typed() {
    let fx = fixture();
    let mut app = fx.scanned_app();
    let path = queue_from_overview(&mut app, "code/rs/target");
    press(&mut app, KeyCode::Char('5'));
    press(&mut app, KeyCode::Char('m'));
    assert!(screen(&mut app, 80, 24).contains("PERMANENT"));
    press(&mut app, KeyCode::Char('D'));
    let total = match &app.cleanup.phase {
        Phase::Confirming { ready, .. } => ready.total_human.clone(),
        _ => panic!("permanent mode asks for the total first"),
    };
    let s = screen(&mut app, 80, 24);
    assert!(s.contains("Type the total to confirm"), "{s}");

    type_str(&mut app, "1 B");
    press(&mut app, KeyCode::Enter);
    assert!(matches!(
        app.cleanup.phase,
        Phase::Confirming { wrong: true, .. }
    ));
    assert!(screen(&mut app, 100, 30).contains("Nothing was deleted"));
    std::thread::sleep(Duration::from_millis(100));
    assert!(path.exists(), "a wrong total deletes nothing");

    for _ in 0..3 {
        press(&mut app, KeyCode::Backspace);
    }
    type_str(&mut app, &total);
    press(&mut app, KeyCode::Enter);
    wait_for_cleanup(&mut app);
    let r = report(&app);
    assert_eq!(r.results[0].outcome, Outcome::Deleted);
    assert!(r.results[0].trashed_to.is_none());
    assert!(!path.exists());
}

#[test]
fn review_items_need_their_own_confirmation() {
    let fx = fixture();
    let mut app = fx.scanned_app();
    let dmg = queue_from_overview(&mut app, "Downloads/setup.dmg");
    let review = session(&app)
        .overview
        .top
        .iter()
        .find(|i| i.path == dmg.to_string_lossy())
        .unwrap()
        .safety;
    assert_eq!(review, spacehog_core::rules::Safety::Review);
    press(&mut app, KeyCode::Char('5'));
    press(&mut app, KeyCode::Char('D'));
    let s = screen(&mut app, 80, 24);
    assert!(s.contains("Review item 1 of 1"), "{s}");
    press(&mut app, KeyCode::Char('n'));
    assert!(matches!(app.cleanup.phase, Phase::Browse));
    assert!(dmg.exists(), "declined review items are kept");

    press(&mut app, KeyCode::Char('D'));
    press(&mut app, KeyCode::Char('y'));
    wait_for_cleanup(&mut app);
    assert!(!dmg.exists());
}

#[test]
fn treemap_zooms_in_and_out_with_the_breadcrumb() {
    let fx = fixture();
    let mut app = fx.scanned_app();
    press(&mut app, KeyCode::Char('3'));
    let s = screen(&mut app, 120, 36);
    assert_eq!(breadcrumb(&s).split_whitespace().next(), Some("~"), "{s}");
    assert!(s.contains("code/"), "top-level tiles are labelled:\n{s}");
    assert!(s.contains("depth 2"), "{s}");
    assert_eq!(session(&app).treemap.selected().unwrap().name, "code");

    press(&mut app, KeyCode::Enter);
    assert!(session(&app).treemap.animating(), "zoom animates");
    std::thread::sleep(spacehog_tui::treemap::ZOOM + Duration::from_millis(20));
    app.tick().unwrap();
    assert!(!session(&app).treemap.animating());
    let s = screen(&mut app, 80, 24);
    assert!(breadcrumb(&s).starts_with("~ › code"), "{s}");
    assert!(s.contains("rs/"), "{s}");

    press(&mut app, KeyCode::Char('-'));
    assert_eq!(session(&app).treemap.depth, 1);
    press(&mut app, KeyCode::Char('+'));
    press(&mut app, KeyCode::Char('+'));
    assert_eq!(session(&app).treemap.depth, 3);

    press(&mut app, KeyCode::Backspace);
    std::thread::sleep(spacehog_tui::treemap::ZOOM + Duration::from_millis(20));
    app.tick().unwrap();
    let s = screen(&mut app, 120, 36);
    assert!(!breadcrumb(&s).contains("code"), "{s}");
    assert_eq!(
        session(&app).treemap.selected().unwrap().name,
        "code",
        "zooming out lands on the folder it came from"
    );
}

#[test]
fn changes_compares_two_scans_and_explains_a_single_one() {
    let fx = fixture();
    let mut app = fx.scanned_app();
    press(&mut app, KeyCode::Char('6'));
    for (w, h) in [(80, 24), (160, 40)] {
        let s = screen(&mut app, w, h);
        assert!(s.contains("Nothing to compare yet"), "{s}");
        assert!(
            s.to_lowercase()
                .contains("rescan later to see what changed"),
            "{s}"
        );
    }

    write(&fx.home.join("Downloads/big.iso"), 3_000_000);
    write(
        &fx.home.join("code/web/node_modules/huge/blob.bin"),
        2_000_000,
    );
    press(&mut app, KeyCode::Char('r'));
    press(&mut app, KeyCode::Esc);
    wait_for_scan(&mut app);
    assert_eq!(app.view, View::Changes);
    let small = screen(&mut app, 80, 24);
    assert!(small.contains("Growth by bucket"), "{small}");
    assert!(small.contains("Grew"), "{small}");
    let wide = screen(&mut app, 160, 40);
    assert!(wide.contains("~/Downloads/big.iso"), "{wide}");
    assert!(
        wide.contains("Dev caches") || wide.contains("Downloads"),
        "{wide}"
    );
    assert!(wide.contains("Shrank"), "{wide}");
    let d = session(&app).changes.diff.as_ref().unwrap();
    assert!(d.total_delta >= 5_000_000, "{}", d.total_delta);
    assert!(wide.contains(&d.total_delta_human), "{wide}");

    press(&mut app, KeyCode::Char('['));
    assert!(app.flash.as_deref().unwrap_or("").contains("no older scan"));
}

#[test]
fn changes_flag_opens_the_changes_view() {
    let fx = fixture();
    fx.scanned_app();
    let mut app = fx.app(&TuiOptions {
        changes: true,
        ..Default::default()
    });
    assert_eq!(app.view, View::Changes);
    assert!(screen(&mut app, 80, 24).contains("Nothing to compare yet"));
}

fn every_view(app: &mut App, w: u16, h: u16) -> Vec<ratatui::buffer::Buffer> {
    let mut out = Vec::new();
    for v in ['1', '2', '3', '4', '5', '6'] {
        press(app, KeyCode::Char(v));
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| ui::draw(f, app)).unwrap();
        out.push(term.backend().buffer().clone());
    }
    out
}

#[test]
fn no_color_paints_no_colour_but_keeps_glyphs() {
    let fx = fixture();
    let mut app = fx.scanned_app();
    queue_from_overview(&mut app, "code/rs/target");
    app.theme = Theme::resolve(
        &Config::default(),
        false,
        Some("1".as_ref()),
        Some("truecolor"),
    );
    assert_eq!(app.theme.tone, Tone::Mono);
    press(&mut app, KeyCode::Char('2'));
    press(&mut app, KeyCode::Char('p'));
    for buf in every_view(&mut app, 120, 36) {
        for cell in buf.content() {
            assert_eq!(
                (cell.fg, cell.bg),
                (ratatui::style::Color::Reset, ratatui::style::Color::Reset),
                "{cell:?}"
            );
        }
    }
    press(&mut app, KeyCode::Char('1'));
    let s = screen(&mut app, 120, 36);
    assert!(
        s.contains("█") && s.contains("·"),
        "bars and tracks stay readable:\n{s}"
    );
    press(&mut app, KeyCode::Char('3'));
    let s = screen(&mut app, 120, 36);
    assert!(
        s.contains("│") && s.contains("─"),
        "tiles are outlined:\n{s}"
    );
    assert!(
        Theme::resolve(&Config::default(), false, Some("".as_ref()), None).tone == Tone::Dark,
        "an empty NO_COLOR does not count"
    );
}

#[test]
fn light_theme_differs_from_dark() {
    let fx = fixture();
    let mut app = fx.scanned_app();
    let dark = every_view(&mut app, 100, 30);
    let config = Config {
        theme: "light".into(),
        ..Config::default()
    };
    app.theme = Theme::resolve(&config, false, None, Some("truecolor"));
    assert_eq!(app.theme.tone, Tone::Light);
    let light = every_view(&mut app, 100, 30);
    let bg = app.theme.hex(spacehog_tui::theme::LIGHT_BG);
    for (d, l) in dark.iter().zip(&light) {
        assert_ne!(d, l);
        let symbols = |b: &ratatui::buffer::Buffer| -> String {
            b.content().iter().map(|c| c.symbol()).collect()
        };
        assert_eq!(symbols(d), symbols(l), "same content, different paint");
        assert!(
            l.content()
                .iter()
                .filter(|c| c.symbol() == " ")
                .all(|c| c.bg != ratatui::style::Color::Reset),
            "the light theme paints its own background"
        );
        assert!(l.content().iter().any(|c| c.bg == bg));
    }
}

#[test]
fn mouse_clicks_filter_the_overview_and_drill_in() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    use spacehog_tui::app::Hit;
    let fx = fixture();
    let mut app = fx.scanned_app();
    let click = |app: &mut App, col: u16, row: u16| {
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        })
        .unwrap();
    };
    screen(&mut app, 160, 40);
    let (rect, _) = *app
        .hits
        .iter()
        .find(|(_, h)| *h == Hit::Bucket(Some(1)))
        .expect("legend rows are clickable");
    click(&mut app, rect.x + 2, rect.y);
    assert_eq!(session(&app).overview.bucket, Some(1));
    let id = session(&app).overview.buckets[1].id.clone();
    assert!(session(&app).overview.top.iter().all(|i| i.bucket == id));

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 100,
        row: 20,
        modifiers: KeyModifiers::NONE,
    })
    .unwrap();

    let (tab, _) = *app
        .hits
        .iter()
        .find(|(_, h)| *h == Hit::View(View::Explorer))
        .unwrap();
    click(&mut app, tab.x + 1, tab.y);
    assert_eq!(app.view, View::Explorer);
    let s = screen(&mut app, 120, 36);
    let row_of = |s: &str, name: &str| s.lines().position(|l| l.contains(name)).unwrap() as u16;
    click(&mut app, 10, row_of(&s, "Library/"));
    assert_eq!(session(&app).explorer.selected().unwrap().name, "Library");
    let row = row_of(&s, "code/");
    click(&mut app, 10, row);
    assert_eq!(session(&app).explorer.selected().unwrap().name, "code");
    click(&mut app, 10, row);
    assert_eq!(
        session(&app).explorer.selected().unwrap().name,
        "rs",
        "a second click drills in"
    );

    press(&mut app, KeyCode::Char('3'));
    screen(&mut app, 120, 36);
    let area = session(&app).treemap.area;
    click(&mut app, area.x + 2, area.y + 2);
    assert!(
        session(&app).treemap.trail.len() == 2,
        "clicking a tile zooms in"
    );
}

#[test]
fn help_and_palette_list_every_cleanup_action() {
    let fx = fixture();
    let mut app = fx.scanned_app();
    press(&mut app, KeyCode::Char('5'));
    press(&mut app, KeyCode::Char('?'));
    let s = screen(&mut app, 100, 36);
    for label in [
        "Approve proposal",
        "Clean up now",
        "Toggle Trash / permanent",
        "Undo the last cleanup",
    ] {
        assert!(s.contains(label), "{label}:\n{s}");
    }
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char(':'));
    type_str(&mut app, "perman");
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        app.cleanup.mode,
        spacehog_core::config::DeleteMode::Permanent
    );
}
