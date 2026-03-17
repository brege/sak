use std::{
    fmt::Write,
    fs::OpenOptions,
    io::IsTerminal,
    sync::{Arc, Mutex, Once},
    time::{Duration, Instant},
};

use bytesize::ByteSize;
use indicatif::{HumanDuration, ProgressBar, ProgressState, ProgressStyle};
use log::{LevelFilter, info};
use rustic_core::{Progress, ProgressBars, ProgressType, RusticProgress};
use simplelog::{ColorChoice, Config, TermLogger, TerminalMode, WriteLogger};

const INTERACTIVE_INTERVAL: Duration = Duration::from_millis(100);
const LOG_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy)]
pub struct UiProgress;

impl UiProgress {
    fn create_progress(self, prefix: &str, kind: ProgressType) -> Progress {
        if std::io::stderr().is_terminal() {
            Progress::new(InteractiveProgress::new(prefix, kind))
        } else {
            Progress::new(NonInteractiveProgress::new(prefix, kind))
        }
    }
}

impl ProgressBars for UiProgress {
    fn progress(&self, kind: ProgressType, prefix: &str) -> Progress {
        self.create_progress(prefix, kind)
    }
}

#[derive(Debug, Clone)]
struct InteractiveProgress {
    bar: ProgressBar,
    kind: ProgressType,
}

impl InteractiveProgress {
    fn new(prefix: &str, kind: ProgressType) -> Self {
        let bar = ProgressBar::new(0).with_style(Self::initial_style(kind));
        bar.set_prefix(prefix.to_string());
        bar.enable_steady_tick(INTERACTIVE_INTERVAL);
        Self { bar, kind }
    }

    #[allow(clippy::literal_string_with_formatting_args)]
    fn initial_style(kind: ProgressType) -> ProgressStyle {
        let template = match kind {
            ProgressType::Spinner => "[{elapsed_precise}] {prefix:30} {spinner}",
            ProgressType::Counter => "[{elapsed_precise}] {prefix:30} {bar:40.cyan/blue} {pos:>10}",
            ProgressType::Bytes => {
                "[{elapsed_precise}] {prefix:30} {bar:40.cyan/blue} {bytes:>10}            {bytes_per_sec:12}"
            }
        };
        ProgressStyle::default_bar().template(template).unwrap()
    }

    #[allow(clippy::literal_string_with_formatting_args)]
    fn style_with_length(kind: ProgressType) -> ProgressStyle {
        match kind {
            ProgressType::Spinner => Self::initial_style(kind),
            ProgressType::Counter => ProgressStyle::default_bar()
                .template("[{elapsed_precise}] {prefix:30} {bar:40.cyan/blue} {pos:>10}/{len:10}")
                .unwrap(),
            ProgressType::Bytes => ProgressStyle::default_bar()
                .with_key("my_eta", |s: &ProgressState, w: &mut dyn Write| {
                    let _ = match (s.pos(), s.len()) {
                        (pos, Some(len)) if pos != 0 && len > pos => {
                            let eta_secs = s.elapsed().as_secs() * (len - pos) / pos;
                            write!(w, "{:#}", HumanDuration(Duration::from_secs(eta_secs)))
                        }
                        _ => write!(w, "-"),
                    };
                })
                .template("[{elapsed_precise}] {prefix:30} {bar:40.cyan/blue} {bytes:>10}/{total_bytes:10} {bytes_per_sec:12} (ETA {my_eta})")
                .unwrap(),
        }
    }
}

impl RusticProgress for InteractiveProgress {
    fn is_hidden(&self) -> bool {
        false
    }

    fn set_length(&self, len: u64) {
        if matches!(self.kind, ProgressType::Bytes | ProgressType::Counter) {
            self.bar.set_style(Self::style_with_length(self.kind));
        }
        self.bar.set_length(len);
    }

    fn set_title(&self, title: &str) {
        self.bar.set_prefix(title.to_string());
    }

    fn inc(&self, inc: u64) {
        self.bar.inc(inc);
    }

    fn finish(&self) {
        self.bar.finish_with_message("done");
    }
}

#[derive(Debug)]
struct NonInteractiveState {
    prefix: String,
    position: u64,
    length: Option<u64>,
    last_log: Instant,
}

#[derive(Clone, Debug)]
struct NonInteractiveProgress {
    state: Arc<Mutex<NonInteractiveState>>,
    start: Instant,
    kind: ProgressType,
}

impl NonInteractiveProgress {
    fn new(prefix: &str, kind: ProgressType) -> Self {
        let now = Instant::now();
        Self {
            state: Arc::new(Mutex::new(NonInteractiveState {
                prefix: prefix.to_string(),
                position: 0,
                length: None,
                last_log: now,
            })),
            start: now,
            kind,
        }
    }

    fn format_value(&self, value: u64) -> String {
        match self.kind {
            ProgressType::Bytes => ByteSize(value).to_string(),
            ProgressType::Counter | ProgressType::Spinner => value.to_string(),
        }
    }

    fn log_progress(&self, state: &NonInteractiveState) {
        let progress = state.length.map_or_else(
            || self.format_value(state.position),
            |len| {
                format!(
                    "{} / {}",
                    self.format_value(state.position),
                    self.format_value(len)
                )
            },
        );
        info!("{}: {}", state.prefix, progress);
    }
}

impl RusticProgress for NonInteractiveProgress {
    fn is_hidden(&self) -> bool {
        false
    }

    fn set_length(&self, len: u64) {
        if let Ok(mut state) = self.state.lock() {
            state.length = Some(len);
        }
    }

    fn set_title(&self, title: &str) {
        if let Ok(mut state) = self.state.lock() {
            state.prefix = title.to_string();
        }
    }

    fn inc(&self, inc: u64) {
        if let Ok(mut state) = self.state.lock() {
            state.position += inc;
            if state.last_log.elapsed() >= LOG_INTERVAL {
                self.log_progress(&state);
                state.last_log = Instant::now();
            }
        }
    }

    fn finish(&self) {
        let Ok(state) = self.state.lock() else {
            return;
        };
        info!(
            "{}: {} done in {:.2?}",
            state.prefix,
            self.format_value(state.position),
            self.start.elapsed()
        );
    }
}

pub fn init_logging() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = TermLogger::init(
            LevelFilter::Info,
            Config::default(),
            TerminalMode::Mixed,
            ColorChoice::Auto,
        );
    });
}

pub fn init_server_logging() -> std::io::Result<()> {
    static INIT: Once = Once::new();
    let mut init_err = None;
    INIT.call_once(|| {
        let path =
            std::env::var("SAK_SERVER_LOG").unwrap_or_else(|_| "/tmp/sak-server.log".to_string());
        match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(file) => {
                let _ = WriteLogger::init(LevelFilter::Info, Config::default(), file);
            }
            Err(err) => {
                init_err = Some(err);
            }
        }
    });
    if let Some(err) = init_err {
        return Err(err);
    }
    Ok(())
}
