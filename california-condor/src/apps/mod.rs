use std::{
    io::{self, BufWriter, IsTerminal, Write, stderr, stdout},
    sync::{Arc, atomic::AtomicBool, mpsc::Receiver},
};

use andean_condor::core::sequence::SequenceStatus;
use anyhow::Result;
use ratatui::{
    Frame,
    Terminal,
    crossterm::{
        self,
        execute,
        terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
    },
    prelude::CrosstermBackend,
};

pub mod benchmarker;
pub mod noise_detection;
pub mod parallel_encoder;
pub mod quality_check;
pub mod scene_concatenator;
pub mod scene_detection;
pub mod shared_progress;
pub mod target_quality;

pub type PanicHook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send + 'static>;
pub type StdOutOrErrTerminal = Terminal<CrosstermBackend<BufWriter<Box<dyn Write + Send>>>>;

/// When `CONDOR_TEST_MODE=1` is set (testing / automation), skip
/// terminal-specific operations (raw mode, alternate screen, keyboard input)
/// and use JSON progress output instead.
pub fn is_test_mode() -> bool {
    std::env::var("CONDOR_TEST_MODE").is_ok_and(|v| v == "1")
}

pub trait TuiApp: Send + Sync + 'static {
    /// The struct must have a field to store original panic hook as
    /// Option<PanicHook>
    fn original_panic_hook(&mut self) -> &mut Option<PanicHook>;

    fn init(&mut self) -> Result<StdOutOrErrTerminal> {
        let use_stdout = stdout().is_terminal();

        if !is_test_mode() {
            enable_raw_mode()?;
        }

        let writer: Box<dyn Write + Send> = if is_test_mode() {
            Box::new(io::sink())
        } else if use_stdout {
            Box::new(stdout())
        } else {
            Box::new(stderr())
        };
        let mut writer = BufWriter::new(writer);

        if !is_test_mode() {
            execute!(writer, EnterAlternateScreen)?;
        }

        let original_hook = std::panic::take_hook();
        *self.original_panic_hook() = Some(original_hook);
        std::panic::set_hook(Box::new(move |panic_info| {
            if !is_test_mode() {
                let _ = disable_raw_mode();
                let mut w: Box<dyn Write + Send> = if use_stdout {
                    Box::new(stdout())
                } else {
                    Box::new(stderr())
                };
                let _ = execute!(w, LeaveAlternateScreen);
                let _ = execute!(w, crossterm::cursor::Show);
            }
            println!("{:?}", panic_info);
        }));

        let backend = CrosstermBackend::new(writer);
        Ok(Terminal::new(backend)?)
    }

    fn restore(&mut self, mut terminal: StdOutOrErrTerminal) -> Result<()> {
        if !is_test_mode() {
            disable_raw_mode()?;
            execute!(io::stdout(), LeaveAlternateScreen)
                .or_else(|_| execute!(io::stderr(), LeaveAlternateScreen))?;

            let (_columns, rows) = crossterm::terminal::size().unwrap_or((80, 24));
            for _ in 0..rows {
                println!();
            }
            terminal.clear()?;
            terminal.draw(|f| self.render(f))?;
            execute!(io::stdout(), crossterm::cursor::Show)
                .or_else(|_| execute!(io::stderr(), crossterm::cursor::Show))?;
        }

        let _ = std::panic::take_hook();
        if let Some(original) = self.original_panic_hook().take() {
            std::panic::set_hook(original);
        }

        Ok(())
    }

    fn run(
        &mut self,
        progress_rx: Receiver<SequenceStatus>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<()>;

    fn render(&self, frame: &mut Frame);
}
