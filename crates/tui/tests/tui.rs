use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use spacehog_core::Index;
use spacehog_core::config::Config;
use spacehog_core::report::{self, TopQuery};
use spacehog_core::rules::RuleSet;
use spacehog_tui::app::Overlay;
use spacehog_tui::keys::View;
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

/// A fake home covering several buckets, with one stale project.
fn fixture() -> Fixture {
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
        App::new(
            self.ctx(),
            Theme {
                truecolour: true,
                icons: false,
            },
            opts,
        )
        .unwrap()
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
    assert!(breadcrumb(&s).starts_with("~ › "), "{s}");
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
        breadcrumb(&s).contains("~ › code › rs"),
        "largest child of code first:\n{s}"
    );

    press(&mut app, KeyCode::Right);
    let s = screen(&mut app, 120, 30);
    assert!(breadcrumb(&s).contains("~ › code › rs › target"), "{s}");
    assert!(
        s.lines()
            .any(|l| l.contains("target/") && l.contains("Dev builds")),
        "{s}"
    );

    press(&mut app, KeyCode::Left);
    press(&mut app, KeyCode::Backspace);
    let s = screen(&mut app, 80, 24);
    assert_eq!(breadcrumb(&s).trim_end(), "~ › code", "{s}");
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
    assert!(s.contains("▕"), "treemap tiles drawn:\n{s}");
    let narrow = screen(&mut app, 79, 24);
    assert!(
        !narrow.contains("▕"),
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
    assert!(breadcrumb(&s).contains("code › rs › target"), "{s}");
}

#[test]
fn view_switching_and_back() {
    let fx = fixture();
    let mut app = fx.scanned_app();
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.view, View::Explorer);
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.view, View::Overview);
    press(&mut app, KeyCode::Char('3'));
    assert_eq!(app.view, View::Treemap);
    assert!(screen(&mut app, 80, 24).contains("Treemap is not built yet"));
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
    press(&mut app, KeyCode::Char('d'));
    let first = PathBuf::from(
        &app.session
            .as_ref()
            .unwrap()
            .explorer
            .selected()
            .unwrap()
            .path,
    );
    assert_eq!(app.queue, vec![second, first]);
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
    assert!(s.contains("phase"), "{s}");
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
