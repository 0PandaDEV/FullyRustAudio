use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    widgets::{Block, Borders, Gauge, Paragraph},
    Terminal,
};
use rodio::{Decoder, OutputStream, Sink, Source};
use std::{
    error::Error,
    fs::File,
    io::BufReader,
    sync::{Arc, Mutex},
    time::Duration,
};

struct AudioPlayer {
    sink: Arc<Mutex<Sink>>,
    duration: Duration,
    progress: Arc<Mutex<Duration>>,
}

fn main() -> Result<(), Box<dyn Error>> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Setup audio
    let (_stream, stream_handle) = OutputStream::try_default().expect("Failed to get default output device");
    let sink = Sink::try_new(&stream_handle).expect("Failed to create sink");
    let sink = Arc::new(Mutex::new(sink));

    let file = BufReader::new(File::open("outaspace.flac").expect("Failed to open file"));
    let source = Decoder::new(file).expect("Failed to create decoder");
    let duration = source.total_duration().unwrap_or(Duration::from_secs(0));
    sink.lock().unwrap().append(source);

    let audio_player = AudioPlayer {
        sink,
        duration,
        progress: Arc::new(Mutex::new(Duration::from_secs(0))),
    };

    // Run app
    let res = run_app(&mut terminal, audio_player);

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("{:?}", err)
    }

    Ok(())
}

fn run_app(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, mut audio_player: AudioPlayer) -> Result<(), Box<dyn Error>> {
    loop {
        terminal.draw(|f| ui(f, &audio_player))?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Char('q') => return Ok(()),
                KeyCode::Char(' ') => {
                    if audio_player.sink.lock().unwrap().is_paused() {
                        audio_player.sink.lock().unwrap().play();
                    } else {
                        audio_player.sink.lock().unwrap().pause();
                    }
                }
                KeyCode::Left => {
                    let current_pos = *audio_player.progress.lock().unwrap();
                    let new_pos = current_pos.saturating_sub(Duration::from_secs(5));
                    seek(&mut audio_player, new_pos);
                }
                KeyCode::Right => {
                    let current_pos = *audio_player.progress.lock().unwrap();
                    let new_pos = (current_pos + Duration::from_secs(5)).min(audio_player.duration);
                    seek(&mut audio_player, new_pos);
                }
                _ => {}
            }
        }

        // Update progress
        *audio_player.progress.lock().unwrap() += Duration::from_millis(100);
        if *audio_player.progress.lock().unwrap() > audio_player.duration {
            *audio_player.progress.lock().unwrap() = audio_player.duration;
        }

        std::thread::sleep(Duration::from_millis(100));
    }
}

fn ui(f: &mut ratatui::Frame, audio_player: &AudioPlayer) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(2)
        .constraints(
            [
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Min(0),
            ]
            .as_ref(),
        )
        .split(f.size());

    let progress = *audio_player.progress.lock().unwrap();
    let duration = audio_player.duration;
    let progress_percent = (progress.as_secs_f64() / duration.as_secs_f64() * 100.0) as u16;

    let gauge = Gauge::default()
        .block(Block::default().title("Progress").borders(Borders::ALL))
        .gauge_style(Style::default().fg(Color::Yellow))
        .percent(progress_percent);

    f.render_widget(gauge, chunks[0]);

    let controls = Paragraph::new("Space: Play/Pause | Left/Right: Seek | Q: Quit")
        .block(Block::default().borders(Borders::ALL));

    f.render_widget(controls, chunks[1]);
}

fn seek(audio_player: &mut AudioPlayer, position: Duration) {
    let sink = &mut audio_player.sink.lock().unwrap();
    sink.stop();
    let file = BufReader::new(File::open("outaspace.flac").expect("Failed to open file"));
    let source = Decoder::new(file).expect("Failed to create decoder");
    let skipped_source = source.skip_duration(position);
    sink.append(skipped_source);
    *audio_player.progress.lock().unwrap() = position;
}
