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
    f32::consts::PI,
    fs::File,
    io::BufReader,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

struct AudioPlayer {
    sink: Arc<Mutex<Sink>>,
    duration: Duration,
    progress: Arc<Mutex<Duration>>,
    eq_enabled: Arc<AtomicBool>,
}

struct Equalizer<S>
where
    S: Source<Item = f32>,
{
    source: S,
    filters: Vec<BiquadFilter>,
}

struct BiquadFilter {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl BiquadFilter {
    fn new(frequency: f32, q: f32, gain: f32, sample_rate: u32) -> Self {
        let omega = 2.0 * PI * frequency / sample_rate as f32;
        let alpha = omega.sin() / (2.0 * q);
        let a = 10.0f32.powf(gain / 40.0);

        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * omega.cos();
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * omega.cos();
        let a2 = 1.0 - alpha / a;

        BiquadFilter {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        let output = self.b0 * input + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = input;
        self.y2 = self.y1;
        self.y1 = output;
        output
    }
}

impl<S> Equalizer<S>
where
    S: Source<Item = f32>,
{
    fn new(source: S, gains: Vec<f32>) -> Self {
        let sample_rate = source.sample_rate();
        let frequencies = [
            32.0, 64.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
        ];
        let filters = frequencies
            .iter()
            .zip(gains.iter())
            .map(|(&freq, &gain)| BiquadFilter::new(freq, 1.41, gain, sample_rate))
            .collect();

        Equalizer { source, filters }
    }
}

impl<S> Iterator for Equalizer<S>
where
    S: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        self.source.next().map(|sample| {
            self.filters
                .iter_mut()
                .fold(sample, |s, filter| filter.process(s))
        })
    }
}

impl<S> Source for Equalizer<S>
where
    S: Source<Item = f32>,
{
    fn current_frame_len(&self) -> Option<usize> {
        self.source.current_frame_len()
    }

    fn channels(&self) -> u16 {
        self.source.channels()
    }

    fn sample_rate(&self) -> u32 {
        self.source.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.source.total_duration()
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let (_stream, stream_handle) =
        OutputStream::try_default().expect("Failed to get default output device");
    let sink = Sink::try_new(&stream_handle).expect("Failed to create sink");
    let sink = Arc::new(Mutex::new(sink));

    let file = BufReader::new(File::open("outaspace.flac").expect("Failed to open file"));
    let decoder = Decoder::new(file)
        .expect("Failed to create decoder")
        .convert_samples::<f32>();
    let duration = decoder.total_duration().unwrap_or(Duration::from_secs(0));

    let db_gains = vec![4.6, 8.0, 4.6, 0.9, 0.0, 3.0, 0.9, 0.0, 0.0, 0.0];
    let equalizer = Equalizer::new(decoder, db_gains);
    sink.lock().unwrap().append(equalizer);

    let audio_player = AudioPlayer {
        sink,
        duration,
        progress: Arc::new(Mutex::new(Duration::from_secs(0))),
        eq_enabled: Arc::new(AtomicBool::new(true)),
    };

    let res = run_app(&mut terminal, audio_player);

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

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    mut audio_player: AudioPlayer,
) -> Result<(), Box<dyn Error>> {
    loop {
        terminal.draw(|f| ui(f, &audio_player))?;

        if event::poll(Duration::from_millis(10))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => return Ok(()),
                    KeyCode::Char(' ') => {
                        let sink = audio_player.sink.lock().unwrap();
                        if sink.is_paused() {
                            sink.play();
                        } else {
                            sink.pause();
                        }
                    }
                    KeyCode::Char('e') => {
                        let current_pos = *audio_player.progress.lock().unwrap();
                        seek(&mut audio_player, current_pos, true);
                    }
                    KeyCode::Left => {
                        let current_pos = *audio_player.progress.lock().unwrap();
                        let new_pos = current_pos.saturating_sub(Duration::from_secs(5));
                        seek(&mut audio_player, new_pos, false);
                    }
                    KeyCode::Right => {
                        let current_pos = *audio_player.progress.lock().unwrap();
                        let new_pos =
                            (current_pos + Duration::from_secs(5)).min(audio_player.duration);
                        seek(&mut audio_player, new_pos, false);
                    }
                    _ => {}
                }
            }
        }

        {
            let sink = audio_player.sink.lock().unwrap();
            if !sink.is_paused() {
                let mut progress = audio_player.progress.lock().unwrap();
                *progress += Duration::from_millis(10);
                if *progress > audio_player.duration {
                    *progress = audio_player.duration;
                }
            }
        }

        std::thread::sleep(Duration::from_millis(10));
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
                Constraint::Length(3),
                Constraint::Min(0),
            ]
            .as_ref(),
        )
        .split(f.area());

    let progress = *audio_player.progress.lock().unwrap();
    let duration = audio_player.duration;

    let current_secs = progress.as_secs();
    let total_secs = duration.as_secs();

    let current = format!("{}:{:02}", current_secs / 60, current_secs % 60);
    let total = format!("{}:{:02}", total_secs / 60, total_secs % 60);

    let progress_text = format!("{} / {}", current, total);

    let progress_ratio = if duration.as_secs_f32() == 0.0 {
        0.0
    } else {
        progress.as_secs_f32() / duration.as_secs_f32()
    };

    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title("Playback"))
        .gauge_style(Style::default().fg(Color::White))
        .ratio(progress_ratio.into());

    f.render_widget(gauge, chunks[0]);

    let progress_paragraph = Paragraph::new(progress_text)
        .style(Style::default().fg(Color::White))
        .alignment(ratatui::layout::Alignment::Right);

    f.render_widget(progress_paragraph, chunks[0]);

    let controls = Paragraph::new("Space: Play/Pause | Left/Right: Seek | E: Toggle EQ | Q: Quit")
        .block(Block::default().borders(Borders::ALL).title("Controls"))
        .alignment(ratatui::layout::Alignment::Center);

    f.render_widget(controls, chunks[1]);

    let eq_status = if audio_player.eq_enabled.load(Ordering::Relaxed) {
        "Enabled"
    } else {
        "Disabled"
    };
    let eq_status_widget = Paragraph::new(eq_status)
        .block(Block::default().borders(Borders::ALL).title("Equalizer"))
        .alignment(ratatui::layout::Alignment::Center);

    f.render_widget(eq_status_widget, chunks[2]);
}

fn seek(audio_player: &mut AudioPlayer, position: Duration, toggle_eq: bool) {
    let sink = &mut audio_player.sink.lock().unwrap();
    let was_playing = !sink.is_paused();

    if !toggle_eq {
        sink.stop();
    }

    let file = BufReader::new(File::open("outaspace.flac").expect("Failed to open file"));
    let decoder = Decoder::new(file)
        .expect("Failed to create decoder")
        .convert_samples::<f32>();

    let db_gains = vec![4.6, 8.0, 4.6, 0.9, 0.0, 3.0, 0.9, 0.0, 0.0, 0.0];

    if toggle_eq {
        let current_state = audio_player.eq_enabled.load(Ordering::Relaxed);
        audio_player
            .eq_enabled
            .store(!current_state, Ordering::Relaxed);
    }

    let source = if audio_player.eq_enabled.load(Ordering::Relaxed) {
        Box::new(Equalizer::new(decoder, db_gains)) as Box<dyn Source<Item = f32> + Send>
    } else {
        Box::new(decoder) as Box<dyn Source<Item = f32> + Send>
    };

    sink.clear();
    sink.append(source.skip_duration(position));
    *audio_player.progress.lock().unwrap() = position;

    if was_playing {
        sink.play();
    }
}
